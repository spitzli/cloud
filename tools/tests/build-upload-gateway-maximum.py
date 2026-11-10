#!/usr/bin/env python3
"""Bounded local maximum-size native upload gate + actual per-process memory cgroup qualification."""
import argparse
import datetime
import hashlib
import http.client
import json
import os
from pathlib import Path
import secrets
import subprocess
import tarfile
import tempfile
import threading
import time
import urllib.parse

ROOT=Path(__file__).resolve().parents[2]
MAX_OUTPUT=512*1024*1024
MAX_RESULT=MAX_OUTPUT+188416
CHUNK=64*1024
ZERO=bytes(CHUNK)
LAYER_DATA=hashlib.shake_256(b"owned maximum-size nonzero layer fixture").digest(CHUNK)


def header(name,size):
    value=tarfile.TarInfo(name);value.size=size;value.mode=0o644
    return value.tobuf(format=tarfile.USTAR_FORMAT)


def zeros(size):
    while size:
        chunk=ZERO[:min(CHUNK,size)];yield chunk;size-=len(chunk)


def layer_data(size):
    while size:
        chunk=LAYER_DATA[:min(CHUNK,size)];yield chunk;size-=len(chunk)


def member(name,payload):
    yield header(name,len(payload));yield payload
    yield from zeros((-len(payload))%512)


def compact(value):return json.dumps(value,separators=(',',':')).encode()
def hashed(chunks):
    value=hashlib.sha256()
    for chunk in chunks:value.update(chunk)
    return value.hexdigest()
def member_size(size):return 512+((size+511)//512)*512


class Image:
    def __init__(self):
        dummy='0'*64
        self.layer_size=MAX_OUTPUT-8192
        for _ in range(3):
            metadata=self.metadata(dummy)
            self.layer_size=MAX_OUTPUT-sum(member_size(len(payload)) for _,payload in metadata)-512-1024
        self.layer_data_size=self.layer_size-512-1024
        assert self.layer_data_size>0 and self.layer_data_size%512==0
        self.layer_sha=hashed(self.layer())
        self.entries=self.metadata(self.layer_sha)
        self.manifest_sha=hashlib.sha256(self.entries[2][1]).hexdigest()
        self.image_size=sum(member_size(len(payload)) for _,payload in self.entries)+512+self.layer_size+1024
        assert self.image_size==MAX_OUTPUT
        self.image_sha=hashed(self.chunks())

    def metadata(self,layer_sha):
        config=compact({'architecture':'amd64','os':'linux','rootfs':{'type':'layers','diff_ids':['sha256:'+layer_sha]},'history':[{'created_by':'owned maximum upload qualification'}]})
        config_sha=hashlib.sha256(config).hexdigest()
        manifest=compact({'schemaVersion':2,'mediaType':'application/vnd.oci.image.manifest.v1+json','config':{'mediaType':'application/vnd.oci.image.config.v1+json','size':len(config),'digest':'sha256:'+config_sha},'layers':[{'mediaType':'application/vnd.oci.image.layer.v1.tar','size':self.layer_size,'digest':'sha256:'+layer_sha}]})
        manifest_sha=hashlib.sha256(manifest).hexdigest()
        index=compact({'schemaVersion':2,'manifests':[{'mediaType':'application/vnd.oci.image.manifest.v1+json','size':len(manifest),'digest':'sha256:'+manifest_sha}]})
        return [('oci-layout',b'{"imageLayoutVersion":"1.0.0"}'),('index.json',index),('blobs/sha256/'+manifest_sha,manifest),('blobs/sha256/'+config_sha,config)]

    def layer(self):
        yield header('owned-zero-padding',self.layer_data_size)
        yield from layer_data(self.layer_data_size)
        yield bytes(1024)

    def chunks(self):
        for name,payload in self.entries:yield from member(name,payload)
        yield header('blobs/sha256/'+self.layer_sha,self.layer_size)
        yield from self.layer()
        yield bytes(1024)

    def result(self,identity):
        attempt=compact(identity);assert len(attempt)<=4096
        status=compact({'success':True,'sourceSha':identity['sourceSha'],'archiveSha256':identity['archiveSha256'],'artifactDigest':'sha256:'+self.image_sha,'imageDigest':'sha256:'+self.manifest_sha,'artifactBytes':MAX_OUTPUT})
        assert len(status)<=4096
        yield from member('attempt.json',attempt.ljust(4096,b' '))
        yield from member('status.json',status.ljust(4096,b' '))
        yield from member('build.log',b'owned maximum-size fixture\n'.ljust(24000,b' '))
        yield from member('events.jsonl',b'{}\n'*(152192//3)+b'\n'*(152192%3))
        yield header('image.tar',MAX_OUTPUT)
        yield from self.chunks()
        yield bytes(1024)


def show(unit):
    result=subprocess.run(['systemctl','--user','show',unit,'--property=ActiveState,Result,MainPID,ControlGroup,MemoryPeak,MemoryCurrent,MemoryMax'],capture_output=True,text=True,timeout=10)
    return dict(line.split('=',1) for line in result.stdout.splitlines() if '=' in line)


def private_write(path,value):
    temporary=path.with_name(path.name+'.tmp')
    fd=os.open(temporary,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
    with os.fdopen(fd,'w') as target:json.dump(value,target)
    temporary.replace(path)


def load(path):
    try:return json.loads(path.read_text())
    except FileNotFoundError:return None


def wait_file(path,predicate,unit,seconds=15):
    until=time.monotonic()+seconds
    while time.monotonic()<until:
        value=load(path)
        if value and predicate(value):return value
        if show(unit).get('ActiveState')!='active':raise RuntimeError('isolated_gateway_exited')
        time.sleep(0.05)
    raise RuntimeError('isolated_gateway_wait_deadline')


def connect(control):
    host,port=control['address'].split(':');connection=http.client.HTTPConnection(host,int(port),timeout=100)
    url=urllib.parse.urlparse(control['grant'])
    connection.putrequest('PUT',url.path+'?'+url.query,skip_accept_encoding=True)
    connection.putheader('Transfer-Encoding','chunked');connection.putheader('Connection','close');connection.endheaders()
    return connection


def chunk(connection,data):
    if data:connection.send(format(len(data),'x').encode()+b'\r\n'+data+b'\r\n')


def case(binary,image,budget):
    work=Path(tempfile.mkdtemp(prefix='webdock-upload-max-'));work.chmod(0o700);(work/'gateway-tmpfs').mkdir(mode=0o700)
    unit='webdock-upload-maximum-'+secrets.token_hex(6)+'.service'
    result={'memoryMaxBytes':budget,'validUploadAccepted':False,'collectorAccepted':False,'exactMaximumResultBytes':MAX_RESULT,'exactMaximumImageBytes':MAX_OUTPUT}
    started=time.monotonic();sample_stop=threading.Event();sample_thread=None;sampled_peak=[0];sampled_cgroup_peak=[0];sampled_events={}
    try:
        script='mount --make-rprivate / && mount -t tmpfs -o size=671088640,mode=0700,nosuid,nodev,noexec owned-upload-maximum "$SPITZLI_UPLOAD_MAX_WORK/gateway-tmpfs" && exec "$1" --exact maximum_gateway_service --ignored --nocapture --test-threads=1'
        subprocess.run(['systemd-run','--user','--quiet','--unit='+unit,'--property=MemoryMax='+str(budget),'--property=MemorySwapMax=0','--property=TasksMax=16','--property=CPUQuota=200%','--property=RuntimeMaxSec=180','--property=TimeoutStopSec=5','--property=OOMPolicy=stop','--property=StandardOutput=null','--property=StandardError=null','--setenv=SPITZLI_UPLOAD_MAX_WORK='+str(work),'unshare','--user','--map-root-user','--map-auto','--mount','--propagation','private','/bin/bash','-c',script,'owned-maximum-fixture',str(binary)],check=True,capture_output=True,timeout=15)
        control=wait_file(work/'control.json',lambda v:True,unit)
        state=show(unit);pid=int(state['MainPID']);cgroup=Path('/sys/fs/cgroup')/state['ControlGroup'].lstrip('/')
        def sample_rss():
            while not sample_stop.wait(0.02):
                try:
                    text=Path('/proc',str(pid),'status').read_text()
                    peak=int(next(line.split()[1] for line in text.splitlines() if line.startswith('VmHWM:')))*1024
                    sampled_peak[0]=max(sampled_peak[0],peak)
                except (FileNotFoundError,ProcessLookupError,StopIteration):pass
                try:
                    sampled_cgroup_peak[0]=max(sampled_cgroup_peak[0],int((cgroup/'memory.peak').read_text()))
                    sampled_events.update({key:int(value) for key,value in (line.split() for line in (cgroup/'memory.events').read_text().splitlines())})
                except (FileNotFoundError,OSError):return
        sample_thread=threading.Thread(target=sample_rss,daemon=True);sample_thread.start()
        connection=connect(control);sent=0
        try:
            for data in image.result(control['identity']):chunk(connection,data);sent+=len(data)
            assert sent==MAX_RESULT
            connection.send(b'0\r\n\r\n');response=connection.getresponse();result['uploadStatus']=response.status;result['validUploadAccepted']=response.status==200;response.read(128)
        except (OSError,http.client.HTTPException):result['uploadOutcome']='connection_closed'
        finally:connection.close()
        if result['validUploadAccepted']:
            result['ingressPeak']=show(unit).get('MemoryPeak')
            private_write(work/'command.json','collect')
            try:
                progress=wait_file(work/'progress.json',lambda v:v['collected'],unit,seconds=70)
                result['collectorAccepted']=True;result['peakRssBytes']=progress['peakRssBytes']
                second=wait_file(work/'control.json',lambda v:v.get('generation')==2,unit)
                connection=connect(second)
                try:
                    for data in zeros(MAX_RESULT):chunk(connection,data)
                    progress=wait_file(work/'progress.json',lambda v:v['tmpfsUsedBytes']==MAX_RESULT,unit)
                    result['physicalBytesBeforeOverflow']=progress['tmpfsUsedBytes']
                    chunk(connection,b'x');connection.send(b'0\r\n\r\n');response=connection.getresponse();result['overflowStatus']=response.status;response.read(128)
                finally:connection.close()
                private_write(work/'command.json','overflow-done')
                progress=wait_file(work/'progress.json',lambda v:v['overflowRetired'] and v['tmpfsUsedBytes']==0,unit)
                result['exactCleanup']=True;result['peakRssBytes']=progress['peakRssBytes']
            except RuntimeError:result['collectorOutcome']='isolated_gateway_exited_or_deadline'
        time.sleep(0.1)
        observed=show(unit)
        try:result['cgroupMemoryEvents']={key:int(value) for key,value in (line.split() for line in (cgroup/'memory.events').read_text().splitlines())}
        except FileNotFoundError:
            result['cgroupMemoryEventsUnavailableAfterExit']=True
            result['cgroupMemoryEvents']=dict(sampled_events)
        result['independentPeakRssBytes']=sampled_peak[0]
        result['memoryCurrentAfterFileCleanupBytes']=int(observed.get('MemoryCurrent','0')) if observed.get('MemoryCurrent','0').isdigit() else None
        result.update(systemdResult=observed.get('Result'),activeState=observed.get('ActiveState'),memoryPeakBytes=max(int(observed.get('MemoryPeak','0')) if observed.get('MemoryPeak','0').isdigit() else 0,sampled_cgroup_peak[0]))
        if observed.get('ActiveState')=='active':private_write(work/'command.json','quit')
    finally:
        sample_stop.set()
        if sample_thread is not None:sample_thread.join(timeout=1)
        subprocess.run(['systemctl','--user','stop',unit],capture_output=True,timeout=10)
        # Named payloads are forbidden. The namespace closes the anonymous tmpfs on exit/OOM.
        if any((work/'gateway-tmpfs').iterdir()):raise RuntimeError('unexpected_underlying_payload')
        for name in ['control.json','progress.json','command.json','control.json.tmp','progress.json.tmp','command.json.tmp']:
            path=work/name
            if path.exists():path.unlink()
        (work/'gateway-tmpfs').rmdir();work.rmdir()
        subprocess.run(['systemctl','--user','reset-failed',unit],capture_output=True,timeout=10)
    result['elapsedSeconds']=round(time.monotonic()-started,3)
    return result


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--evidence',type=Path,default=ROOT/'docs/verification/2026-10-30-build-upload-maximum-memory.json');args=parser.parse_args()
    available=int(next(line.split()[1] for line in Path('/proc/meminfo').read_text().splitlines() if line.startswith('MemAvailable:')))*1024
    if available<8*1024**3:raise RuntimeError('at_least_8GiB_available_required')
    built=subprocess.run(['cargo','test','-p','spitzli-build','--features','worker,kubernetes-job','--test','upload_gateway_maximum','--no-run','--message-format=json'],cwd=ROOT,capture_output=True,text=True,timeout=120)
    if built.returncode:raise RuntimeError('maximum_fixture_compile_failed')
    binaries=[row['executable'] for row in map(json.loads,built.stdout.splitlines()) if row.get('reason')=='compiler-artifact' and row.get('target',{}).get('name')=='upload_gateway_maximum' and row.get('executable')]
    assert len(binaries)==1
    image=Image();measurements=[]
    for budget in [2*1024**3,4*1024**3]:
        measurement=case(Path(binaries[0]),image,budget);measurements.append(measurement);print(json.dumps(measurement),flush=True)
    report={'verifiedAt':datetime.datetime.now(datetime.timezone.utc).isoformat(),'profile':'maximum-valid-oci-native-http-dedicated-tmpfs-memory-cgroup','rawMaximumBytes':MAX_RESULT,'imageMaximumBytes':MAX_OUTPUT,'tmpfsHardBytes':640*1024**2,'availableMemoryBeforeBytes':available,'measurements':measurements,'maximumAccepted':measurements[-1]['collectorAccepted'],'layerPattern':'deterministic nonzero 64KiB SHAKE256 block; uncompressed regular-file tar layer','recommendation':{'workerMemoryMaxBytes':4*1024**3,'isolatedGatewayMinimumBytes':3*1024**3,'production2GiBHeadroomQualified':False,'reason':'valid nonzero maximum-size OCI can OOM the2GiB gateway; larger bound is needed before accounting for other worker state'},'limits':{'productionWorkerChanged':False,'remoteHostChanged':False,'liveRelayChanged':False,'tenantCodeExecuted':False}}
    args.evidence.parent.mkdir(parents=True,exist_ok=True);args.evidence.write_text(json.dumps(report,indent=2)+'\n')
    assert report['maximumAccepted'] and measurements[-1].get('overflowStatus')==413 and measurements[-1].get('exactCleanup')


if __name__=='__main__':main()
