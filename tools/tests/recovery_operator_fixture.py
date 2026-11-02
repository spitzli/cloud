"""Finite operator inputs for the owned no-byte app authority component; existing MinIO only."""
import hashlib,json,os,re,secrets,selectors,shutil,stat,subprocess,time,urllib.parse,datetime
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]

def digest(path):return hashlib.sha256(Path(path).read_bytes()).hexdigest()
def private(path,limit):
    path=Path(path)
    if path.is_symlink() or path.resolve()!=path:raise ValueError('noncanonical private operator file')
    fd=os.open(path,os.O_RDONLY|os.O_NOFOLLOW)
    try:
        m=os.fstat(fd)
        if not stat.S_ISREG(m.st_mode) or m.st_uid!=os.getuid() or m.st_nlink!=1 or stat.S_IMODE(m.st_mode)!=0o600 or m.st_size>limit:raise ValueError('private operator file ownership')
        raw=os.read(fd,limit+1)
        if len(raw)>limit:raise ValueError('private operator file bound')
        return raw
    finally:os.close(fd)
def write_private(path,value):
    raw=value if isinstance(value,bytes) else json.dumps(value).encode()
    with open(path,'xb') as f:os.chmod(path,0o600);f.write(raw);f.flush();os.fsync(f.fileno())
def user_add_input(access,secret):
    if not access or not secret or any(c in access+secret for c in '\r\n'):raise ValueError('invalid owned principal')
    # Maintained mc RELEASE.2025-08-13 stdin contract; stdout JSON includes SecretKey, so never retain it.
    return (access+'\n'+secret+'\n').encode()
MAX_INVENTORY_BYTES=262144
MAX_INVENTORY_RECORDS=2048
INVENTORY_CATEGORIES={'invalid-kind','invalid-name','byte-bound','failed-exit','unexpected-stderr','invalid-json','invalid-framing','record-bound','invalid-record-status','invalid-bucket-schema','invalid-user-schema','invalid-policy-schema','duplicate-json-key','duplicate-name','input-bound','timeout','process-io','invalid-command','command-bound'}
SAFE_ERROR_CODES={'AccessDenied','InvalidAccessKeyId','SignatureDoesNotMatch','NoSuchBucket','XMinioAdminNoSuchUser','XMinioAdminNoSuchPolicy'}
class InventoryError(RuntimeError):
    def __init__(self,category):
        if category not in INVENTORY_CATEGORIES:category='invalid-record-status'
        self.category=category;super().__init__('unknown provider inventory: '+category)
def unique_object(pairs):
    value={}
    for key,item in pairs:
        if key in value:raise InventoryError('duplicate-json-key')
        value[key]=item
    return value
def canonical_name(kind,name):
    if not isinstance(name,str) or not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.@+=:-]{0,127}',name):raise InventoryError('invalid-name')
    if kind=='bucket' and (not re.fullmatch(r'[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]',name) or any(x in name for x in ('..','.-','-.'))):raise InventoryError('invalid-name')
    return name
def parse_inventory(kind,return_code,stdout,stderr):
    if kind not in ('bucket','user','policy'):raise InventoryError('invalid-kind')
    if len(stdout)+len(stderr)>MAX_INVENTORY_BYTES:raise InventoryError('byte-bound')
    if return_code!=0:raise InventoryError('failed-exit')
    if stderr:raise InventoryError('unexpected-stderr')
    # Measured bucket/user empty exit0; pinned mc list handlers print once per entry,
    # including policy-list, so zero entries produce exactly no bytes (not an error body).
    if stdout==b'':return set()
    def invalid_constant(_):raise InventoryError('invalid-json')
    decoder=json.JSONDecoder(object_pairs_hook=unique_object,parse_constant=invalid_constant)
    try:text=stdout.decode('utf8')
    except UnicodeError:raise InventoryError('invalid-json') from None
    names=set();position=0;count=0
    while position<len(text):
        while position<len(text) and text[position] in ' \t\r\n':position+=1
        if position==len(text):break
        try:row,end=decoder.raw_decode(text,position)
        except (ValueError,RecursionError):raise InventoryError('invalid-json') from None
        if end<len(text) and text[end] not in ' \t\r\n':raise InventoryError('invalid-framing')
        position=end;count+=1
        if count>MAX_INVENTORY_RECORDS:raise InventoryError('record-bound')
        if not isinstance(row,dict) or row.get('status')!='success':raise InventoryError('invalid-record-status')
        if kind=='bucket':
            required={'status','type','lastModified','size','key','etag'}
            if not required<=set(row) or set(row)-required-{'url','versionOrdinal'} or row['type']!='folder' or type(row['size']) is not int or row['size']!=0 or row['etag']!='':raise InventoryError('invalid-bucket-schema')
            if 'versionOrdinal' in row and (type(row['versionOrdinal']) is not int or not 0<=row['versionOrdinal']<=9223372036854775807):raise InventoryError('invalid-bucket-schema')
            key=row['key']
            if not isinstance(key,str) or not key.endswith('/'):raise InventoryError('invalid-bucket-schema')
            name=canonical_name(kind,key[:-1])
            try:
                stamp=row['lastModified']
                if not isinstance(stamp,str) or len(stamp)>40 or datetime.datetime.fromisoformat(stamp.replace('Z','+00:00')).tzinfo is None:raise ValueError('invalid timestamp')
            except (ValueError,TypeError):raise InventoryError('invalid-bucket-schema') from None
            if 'url' in row and (not isinstance(row['url'],str) or len(row['url'])>4096):raise InventoryError('invalid-bucket-schema')
        elif kind=='user':
            required={'status','accessKey','userStatus'}
            if not required<=set(row) or set(row)-required-{'policyName','memberOf','authentication'} or row['userStatus'] not in ('enabled','disabled'):raise InventoryError('invalid-user-schema')
            name=canonical_name(kind,row['accessKey'])
            for field in ('policyName','authentication'):
                if field in row and (not isinstance(row[field],str) or len(row[field])>4096):raise InventoryError('invalid-user-schema')
            groups=row.get('memberOf',[])
            if not isinstance(groups,list) or len(groups)>128:raise InventoryError('invalid-user-schema')
            seen_groups=set()
            for group in groups:
                if not isinstance(group,dict) or set(group)!={'name','policies'} or not isinstance(group['policies'],list) or len(group['policies'])>128:raise InventoryError('invalid-user-schema')
                group_name=canonical_name('user',group['name'])
                if group_name in seen_groups:raise InventoryError('invalid-user-schema')
                seen_groups.add(group_name)
                for policy in group['policies']:canonical_name('policy',policy)
                if len(group['policies'])!=len(set(group['policies'])):raise InventoryError('invalid-user-schema')
        else:
            # Pinned mc userPolicyMessage + madmin PolicyInfo zero-value list encoding.
            if set(row)!={'status','policy','policyInfo','isGroup'} or row['isGroup'] is not False or row['policyInfo']!={'PolicyName':'','Policy':None}:raise InventoryError('invalid-policy-schema')
            name=canonical_name(kind,row['policy'])
        if name in names:raise InventoryError('duplicate-name')
        names.add(name)
    if count==0:raise InventoryError('invalid-framing')
    return names

def safe_error_code(stdout,stderr):
    for raw in (stdout,stderr):
        try:value=json.loads(raw,object_pairs_hook=unique_object)
        except (ValueError,InventoryError,UnicodeError,RecursionError):continue
        if not isinstance(value,dict) or value.get('status')!='error':continue
        error=value.get('error',{})
        if not isinstance(error,dict):continue
        cause=error.get('cause',{});nested=cause.get('error',{}) if isinstance(cause,dict) else {}
        code=error['Code'] if 'Code' in error else (nested.get('Code') if isinstance(nested,dict) else None)
        if code is not None:return code if isinstance(code,str) and code in SAFE_ERROR_CODES else 'unrecognized'
    return 'none'

def bounded_mc(argv,environment,input_bytes,timeout,output_limit=MAX_INVENTORY_BYTES):
    if input_bytes is not None and len(input_bytes)>16384:raise InventoryError('input-bound')
    try:child=subprocess.Popen(argv,stdin=subprocess.PIPE if input_bytes is not None else subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env=environment)
    except OSError:
        failure=InventoryError('process-io');failure.safe_receipt={'returnCode':None,**{name:{'bytes':0,'sha256':hashlib.sha256(b'').hexdigest()} for name in ('stdout','stderr')}}
        raise failure from None
    buffers={'stdout':bytearray(),'stderr':bytearray()};selector=selectors.DefaultSelector();deadline=time.monotonic()+timeout
    try:
        pending_input=memoryview(input_bytes or b'')
        if child.stdin is not None:
            os.set_blocking(child.stdin.fileno(),False)
            if pending_input:selector.register(child.stdin,selectors.EVENT_WRITE,'stdin')
            else:child.stdin.close()
        for name,stream in [('stdout',child.stdout),('stderr',child.stderr)]:os.set_blocking(stream.fileno(),False);selector.register(stream,selectors.EVENT_READ,name)
        while selector.get_map():
            if time.monotonic()>deadline:raise InventoryError('timeout')
            for key,_ in selector.select(min(.1,max(0,deadline-time.monotonic()))):
                if key.data=='stdin':
                    count=os.write(key.fileobj.fileno(),pending_input);pending_input=pending_input[count:]
                    if not pending_input:selector.unregister(key.fileobj);key.fileobj.close()
                    continue
                data=os.read(key.fileobj.fileno(),8192)
                if not data:selector.unregister(key.fileobj);continue
                buffers[key.data]+=data
                if sum(map(len,buffers.values()))>output_limit:raise InventoryError('byte-bound')
        code=child.wait(timeout=max(.01,deadline-time.monotonic()))
        return subprocess.CompletedProcess(argv,code,bytes(buffers['stdout']),bytes(buffers['stderr']))
    except (InventoryError,subprocess.TimeoutExpired,OSError) as error:
        if child.poll() is None:child.kill()
        code=child.wait()
        failure=error if isinstance(error,InventoryError) else InventoryError('timeout' if isinstance(error,subprocess.TimeoutExpired) else 'process-io')
        failure.safe_receipt={'returnCode':code,**{name:{'bytes':len(value),'sha256':hashlib.sha256(value).hexdigest()} for name,value in buffers.items()}}
        raise failure from None
    finally:
        selector.close()
        if child.poll() is None:child.kill();child.wait()
        for stream in (child.stdin,child.stdout,child.stderr):
            if stream is not None and not stream.closed:stream.close()

class OperatorInputs:
    def __init__(self,base,run,tools,byte_mode=False):
        self.base=Path(base);self.directory=self.base/'operator-private';self.directory.mkdir(mode=0o700)
        self.inputs=self.directory/'outer';self.inputs.mkdir(mode=0o700)
        self.tools=tools;self.run=run;self.items=[];self.closed=False;self.byte_mode=byte_mode;self.deadline=time.monotonic()+(720 if byte_mode else 120)
        self.bucket='wd-app-authority-'+secrets.token_hex(16)
        self.users=[('writer','w'+secrets.token_hex(9),secrets.token_hex(32)),('reader','r'+secrets.token_hex(9),secrets.token_hex(32))]
        self.receipt={'run':run,'scope':'existing MinIO exact fresh resources; no source bytes/provider-network qualification','bucket':self.bucket,'items':self.items,'cleanup':'not-started'}
        self.persist()
    @staticmethod
    def service_state(port):
        result=subprocess.run(['docker','ps','--filter','publish='+str(port),'--format','{{.ID}}'],stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=5)
        ids=result.stdout.splitlines()
        if result.returncode or len(ids)!=1:raise RuntimeError('existing MinIO service not uniquely running')
        result=subprocess.run(['docker','inspect','--format','{{json .Id}} {{json .State.Running}} {{json .State.StartedAt}} {{json .Mounts}}',ids[0].decode()],stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=5)
        if result.returncode:raise RuntimeError('existing MinIO identity unavailable')
        # Exact read-only service receipt contains no Config.Env/credentials.
        return {'metadataSHA256':hashlib.sha256(result.stdout).hexdigest(),'port':port}
    def persist(self):
        path=self.base/'operator-custody.json';tmp=path.with_suffix('.new')
        with open(tmp,'wb') as f:os.chmod(tmp,0o600);f.write(json.dumps(self.receipt,indent=2).encode());f.flush();os.fsync(f.fileno())
        os.replace(tmp,path)
    def mc(self,*args,input_bytes=None,allow=False):
        # CLI user-add output contains SecretKey: only sanitized bounded receipts survive.
        if args[:2]==('ls','fixture'):stage='inventory-bucket'
        elif args[:2]==('ls','--recursive'):stage='owned-bucket-empty-check'
        elif args[:2] in (('admin','user'),('admin','policy')) and len(args)>2 and args[2] in ('list','add','create','attach','remove'):stage=args[1]+'-'+args[2]
        elif getattr(self,'byte_mode',False) and args and args[0]=='pipe':
            canary='authority/permission-delete-'+self.run
            if not re.fullmatch('[a-f0-9]{32}',self.run) or args!=('pipe','fixture/'+self.bucket+'/'+canary) or type(input_bytes) is not bytes or input_bytes!=b'' or self.receipt.get('byteCanary',{}).get('objectKey')!=canary or self.receipt['byteCanary'].get('state')!='pending':raise InventoryError('invalid-command')
            stage='owned-zero-canary-pipe'
        elif getattr(self,'byte_mode',False) and args and args[0]=='rm':
            if len(args)!=2 or input_bytes is not None or not args[1].startswith('fixture/'+self.bucket+'/'):raise InventoryError('invalid-command')
            key=args[1][len('fixture/'+self.bucket+'/'):]
            verified=getattr(self,'_verified_delete_keys',set())
            if key not in verified:raise InventoryError('invalid-command')
            verified.remove(key);stage='owned-receipt-verified-rm'
        elif args and args[0] in ('mb','rb'):stage='bucket-'+args[0]
        else:raise InventoryError('invalid-command')
        records=self.receipt.setdefault('commandReceipts',[])
        if len(records)>=128:raise InventoryError('command-bound')
        receipt={'stage':stage,'category':'started','safeErrorCode':'none'};records.append(receipt);self.persist()
        try:
            result=bounded_mc([str(self.tools['mc']['path']),'--json','--no-color','--disable-pager','--config-dir',str(self.directory/'mc'),*args],self.environment,input_bytes,min(30,max(1,self.deadline-time.monotonic())))
        except InventoryError as error:
            receipt.update(getattr(error,'safe_receipt',{}));receipt['category']=error.category;self.persist();raise
        receipt.update({'safeErrorCode':safe_error_code(result.stdout,result.stderr),'returnCode':result.returncode,'category':'success' if result.returncode==0 else 'failed-exit',**{name:{'bytes':len(value),'sha256':hashlib.sha256(value).hexdigest()} for name,value in [('stdout',result.stdout),('stderr',result.stderr)]}})
        self.persist()
        if result.returncode and not allow:raise InventoryError('failed-exit')
        return result
    def inventory(self,kind):
        args=('ls','fixture') if kind=='bucket' else ('admin',kind,'list','fixture')
        result=self.mc(*args,allow=True)
        try:return parse_inventory(kind,result.returncode,result.stdout,result.stderr)
        except InventoryError as error:
            if self.receipt.get('commandReceipts'):
                self.receipt['commandReceipts'][-1]['category']=error.category;self.persist()
            raise
    def absent(self,kind,name):
        canonical_name(kind,name)
        return name not in self.inventory(kind)
    def create(self,kind,name,work):
        if not self.absent(kind,name):raise RuntimeError('foreign provider resource collision')
        item={'kind':kind,'name':name,'state':'pending'};self.items.append(item);self.persist()
        work();item['state']='created';self.persist()
    def prepare(self):
        values=dict(line.split('=',1) for line in private(ROOT/'.dev-local/minio.env',16384).decode().splitlines() if '=' in line)
        port=int(os.environ.get('WEBDOCK_DEV_S3_PORT','55900'))
        if not 1024<=port<=65535:raise ValueError('existing MinIO port')
        self.endpoint=f'http://127.0.0.1:{port}'
        self.environment={**os.environ,'MC_HOST_fixture':f'http://{urllib.parse.quote(values["MINIO_ROOT_USER"],safe="")}:{urllib.parse.quote(values["MINIO_ROOT_PASSWORD"],safe="")}@127.0.0.1:{port}'}
        self.before_service=self.service_state(port)
        self.receipt['existingMinio']=self.before_service
        self.baseline_buckets=sorted(self.inventory('bucket'))
        self.receipt['existingBucketInventorySHA256']=hashlib.sha256(json.dumps(self.baseline_buckets).encode()).hexdigest();self.persist()
        self.create('bucket',self.bucket,lambda:self.mc('mb','fixture/'+self.bucket))
        for role,access,secret in self.users:
            write_private(self.inputs/(role+'.json'),{'accessKeyId':access,'secretAccessKey':secret})
            self.create('user',access,lambda a=access,s=secret:self.mc('admin','user','add','fixture',input_bytes=user_add_input(a,s)))
            policy='wd-app-'+self.run+'-'+role
            actions=['s3:GetObject'] if role=='reader' else ['s3:GetObject','s3:PutObject','s3:AbortMultipartUpload','s3:ListMultipartUploadParts']
            definition={'Version':'2012-10-17','Statement':[{'Effect':'Allow','Action':actions,'Resource':['arn:aws:s3:::'+self.bucket+'/authority/*']}]}
            self.create('policy',policy,lambda p=policy,d=definition:self.mc('admin','policy','create','fixture',p,'/dev/stdin',input_bytes=json.dumps(d).encode()))
            self.mc('admin','policy','attach','fixture',policy,'--user',access)
        self.run_tool('age-keygen',['-o',str(self.inputs/'identity.agekey')])
        os.chmod(self.inputs/'identity.agekey',0o600)
        recipient=self.run_tool('age-keygen',['-y',str(self.inputs/'identity.agekey')]).decode().strip()
        self.run_tool('ssh-keygen',['-q','-t','ed25519','-N','','-f',str(self.directory/'operator')])
        shutil.copyfile(self.directory/'operator.pub',self.inputs/'operator.pub');os.chmod(self.inputs/'operator.pub',0o600)
        if self.byte_mode:
            shutil.copyfile(self.directory/'operator',self.inputs/'operator.key');os.chmod(self.inputs/'operator.key',0o600)
            self.byte_canary='authority/permission-delete-'+self.run
            self.receipt['byteCanary']={'objectKey':self.byte_canary,'sizeBytes':0,'sha256':hashlib.sha256(b'').hexdigest(),'state':'pending'};self.persist()
            self.mc('pipe','fixture/'+self.bucket+'/'+self.byte_canary,input_bytes=b'')
            self.receipt['byteCanary']['state']='created';self.persist()
        destination_endpoint='http://127.0.0.1:55900' if self.byte_mode else self.endpoint
        write_private(self.inputs/'inputs.json',{'version':1,'keyCustodyID':'operator-'+self.run,'ageRecipient':recipient,'destination':{'id':'minio-'+self.run,'endpoint':destination_endpoint,'region':'us-east-1','bucket':self.bucket,'namespace':'authority'}})
        self.receipt['standardOperatorMaterialCreated']=True;self.receipt['providerNetworkQualified']=False;self.persist()
        return self.inputs
    def run_tool(self,name,args):
        try:
            result=subprocess.run([str(self.tools[name]['path']),*args],stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=15)
        except subprocess.TimeoutExpired as error:
            error.output=None;error.stderr=None
            raise RuntimeError('owned operator material timeout; raw output withheld') from None
        if result.returncode:raise RuntimeError('owned operator material stage failed')
        return result.stdout
    def cleanup_byte_objects(self):
        receipts=[self.receipt['byteCanary']]
        path=self.base/'diagnostics/ntfy-byte-objects.json'
        if path.exists():
            proof=json.loads(private(path,1048576))
            if set(proof)!={'version','objects'} or proof['version']!=1 or not isinstance(proof['objects'],list) or not 1<=len(proof['objects'])<=4:raise RuntimeError('unknown owned object receipt')
            receipts+=proof['objects']
        seen=set()
        for receipt in receipts:
            key=receipt['objectKey'];size=receipt['sizeBytes'];expected=receipt['sha256']
            if not isinstance(key,str) or not re.fullmatch(r'authority/[A-Za-z0-9/_-]{1,255}',key) or any(v in ('','.','..') for v in key.split('/')) or key in seen or type(size) is not int or not 0<=size<=8388608 or not re.fullmatch('[a-f0-9]{64}',expected):raise RuntimeError('owned object binding invalid')
            seen.add(key)
            result=bounded_mc([str(self.tools['mc']['path']),'--no-color','--disable-pager','--config-dir',str(self.directory/'mc'),'cat','fixture/'+self.bucket+'/'+key],self.environment,None,30,output_limit=8388609)
            if result.returncode or result.stderr or len(result.stdout)!=size or hashlib.sha256(result.stdout).hexdigest()!=expected:raise RuntimeError('owned object bytes changed; retained')
            if not hasattr(self,'_verified_delete_keys'):self._verified_delete_keys=set()
            self._verified_delete_keys.add(key)
            self.mc('rm','fixture/'+self.bucket+'/'+key)
            if receipt is self.receipt['byteCanary']:receipt['state']='closed'
        self.receipt['byteObjectsDeletedAfterSourceCustodyClosed']=len(receipts);self.persist()
    def cleanup(self,all_source_processes_absent):
        self.deadline=time.monotonic()+120
        if not all_source_processes_absent:
            self.receipt['cleanup']='retained-source-custody-unknown';self.persist();raise RuntimeError('operator resources retained while source custody unknown')
        if not self.items:
            shutil.rmtree(self.directory);self.closed=True;self.receipt['cleanup']='complete-no-provider-acquisitions';self.receipt['obligations']=[];self.persist();return
        if getattr(self,'byte_mode',False):
            partial=self.mc('ls','--recursive','--incomplete','fixture/'+self.bucket,allow=True)
            if partial.returncode or partial.stdout.strip():raise RuntimeError('unknown partial object transfer retained')
            self.cleanup_byte_objects()
        # Unknown objects/partial transfers still retain all keys/resources; never recursive delete.
        listing=self.mc('ls','--recursive','fixture/'+self.bucket,allow=True)
        if listing.returncode!=0:
            if not self.absent('bucket',self.bucket):raise RuntimeError('provider bucket state unknown')
        elif listing.stdout.strip():
            self.receipt['cleanup']='retained-unexpected-object-bytes';self.persist();raise RuntimeError('unexpected operator bucket data retained')
        failures=[]
        ordered=[i for kind in ('user','policy','bucket') for i in reversed(self.items) if i['kind']==kind]
        for item in ordered:
            if item['state']!='created':
                if item['state']=='pending':failures.append('unknown-'+item['kind'])
                continue
            try:
                kind,name=item['kind'],item['name']
                if kind=='user':self.mc('admin','user','remove','fixture',name)
                elif kind=='policy':self.mc('admin','policy','remove','fixture',name)
                else:self.mc('rb','fixture/'+name)
                if not self.absent(kind,name):raise RuntimeError('provider resource remains')
                item['state']='closed';self.persist()
            except Exception:failures.append(item['kind'])
        if failures:
            self.receipt['cleanup']='incomplete';self.receipt['obligations']=failures;self.persist();raise RuntimeError('operator cleanup incomplete')
        buckets=sorted(self.inventory('bucket'))
        if buckets!=self.baseline_buckets:raise RuntimeError('existing provider bucket inventory changed')
        if self.service_state(self.before_service['port'])!=self.before_service:raise RuntimeError('existing MinIO service identity changed')
        shutil.rmtree(self.directory);self.closed=True;self.receipt['cleanup']='complete-private-erased';self.receipt['obligations']=[];self.persist()
