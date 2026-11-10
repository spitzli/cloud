"""Owned, send-only Stalwart configuration. No public SMTP listener or unrestricted egress."""
import ipaddress
import re
import socket
import ssl
import smtplib
import subprocess
import tempfile
from pathlib import Path


def validate_submission(p):
    required={'customerID','hostname','username','password','hookToken','relayHost','relayUsername','relayPassword','transportRevision'}
    if not isinstance(p,dict) or set(p) not in (required,required|{'resourceIDs'}):raise ValueError('Invalid submission profile')
    if any(not isinstance(p[key],str) for key in required):raise ValueError('Invalid submission fields')
    resources=p.get('resourceIDs')
    if resources is not None and (not isinstance(resources,dict) or set(resources)!={'accountID','hookID','routeID','tlsID','certificateID'} or not all(isinstance(value,str) and re.fullmatch(r'[A-Za-z0-9_-]{1,64}',value) for value in resources.values())):raise ValueError('Invalid submission resource mapping')
    if not re.fullmatch(r'[1-9][0-9]{0,18}',p['customerID']) or not re.fullmatch(r'[1-9][0-9]{0,18}',p['transportRevision']):raise ValueError('Invalid submission identity')
    if not re.fullmatch(r'mail-'+p['customerID']+r'\.(?:[a-z0-9-]+\.)+[a-z0-9-]+',p['hostname']) or p['username']!='wd-submit@'+p['hostname']:raise ValueError('Invalid submission host')
    if not re.fullmatch(r'[A-Za-z0-9_-]{32,128}',p['password']) or not re.fullmatch(r'wd_hook_'+p['customerID']+r'_[A-Za-z0-9_-]{43}',p['hookToken']):raise ValueError('Invalid submission credentials')
    if p['relayHost'] not in ('pro.eu.turbo-smtp.com','pro.turbo-smtp.com'):raise ValueError('Unapproved relay')
    for key in ('relayUsername','relayPassword'):
        if not isinstance(p[key],str) or not 1<=len(p[key])<=1024 or not re.fullmatch(r'[\x21-\x7e]+',p[key]):raise ValueError('Invalid relay credentials')


def verify_relay(profile):
    import json
    import sys
    validate_submission(profile)
    # Bound DNS, TLS and AUTH together; no MAIL FROM/RCPT TO/DATA is sent.
    program="""import json,sys,socket,ssl,smtplib,ipaddress,resource
resource.setrlimit(resource.RLIMIT_AS,(268435456,268435456))
p=json.load(sys.stdin)
ips=sorted({r[4][0] for r in socket.getaddrinfo(p['relayHost'],465,type=socket.SOCK_STREAM)})
if not ips or len(ips)>16 or any(not ipaddress.ip_address(v).is_global for v in ips):raise ValueError()
with smtplib.SMTP_SSL(p['relayHost'],465,context=ssl.create_default_context(),timeout=8) as s:
 if s.sock.getpeername()[0] not in ips:raise ValueError()
 s.ehlo('mail.webdock.dev');s.login(p['relayUsername'],p['relayPassword'])
 if s.noop()[0]!=250:raise ValueError()
print(json.dumps(ips))
"""
    result=subprocess.run([sys.executable,'-c',program],input=json.dumps({key:profile[key] for key in ('relayHost','relayUsername','relayPassword')}),capture_output=True,text=True,timeout=20)
    if result.returncode:raise ValueError('Relay authentication was not confirmed')
    return json.loads(result.stdout)


def submission_network_policy(profile,labels,addresses):
    validate_submission(profile)
    if not addresses or len(addresses)>16:raise ValueError('Missing relay addresses')
    networks=[]
    for value in addresses:
        address=ipaddress.ip_address(value)
        if not address.is_global:raise ValueError('Unsafe relay address')
        networks.append({'ipBlock':{'cidr':str(address)+('/32' if address.version==4 else '/128')}})
    gateway={'namespaceSelector':{'matchLabels':{'kubernetes.io/metadata.name':'spitzli-mail-gateway'}},'podSelector':{'matchLabels':{'app':'mail-gateway'}}}
    return {'apiVersion':'networking.k8s.io/v1','kind':'NetworkPolicy','metadata':{'name':'webdock-mail-submission','namespace':'wd-mail-'+profile['customerID'],'labels':labels},'spec':{
        'podSelector':{'matchLabels':{'webdock.dev/mail-customer-id':profile['customerID']}},'policyTypes':['Ingress','Egress'],
        'ingress':[{'from':[gateway],'ports':[{'protocol':'TCP','port':2465}]}],
        'egress':[{'to':networks,'ports':[{'protocol':'TCP','port':465}]},{'to':[gateway],'ports':[{'protocol':'TCP','port':3139}]}]}}


def configure_submission(client,profile,guard):
    """Caller owns the instance, freezes admission, applies egress and verifies after restart."""
    validate_submission(profile)
    def call(method,args):guard();return client.call(method,args)
    def ensure(kind,rows,predicate,desired):
        matches=[row for row in rows if predicate(row)]
        if len(matches)>1:raise ValueError('Ambiguous submission resources')
        expected=(profile.get('resourceIDs') or {}).get({'Account':'accountID','MtaHook':'hookID','MtaRoute':'routeID','MtaTlsStrategy':'tlsID'}[kind])
        if expected and (not matches or matches[0]['id']!=expected):raise ValueError('Submission resource ownership changed')
        if matches:
            if '@type' in desired and matches[0].get('@type')!=desired['@type']:raise ValueError('Submission resource type changed')
            resource_id=matches[0]['id'];call('x:'+kind+'/set',{'update':{resource_id:{key:value for key,value in desired.items() if key not in ('@type','name','domainId')}}})
        else:resource_id=call('x:'+kind+'/set',{'create':{'resource':desired}})['created']['resource']['id']
        return resource_id
    domains=call('x:Domain/get',{'ids':None})['list']
    domain=next((row for row in domains if row['name']==profile['hostname']),None)
    if not domain:raise ValueError('Submission domain is missing')
    accounts=call('x:Account/get',{'ids':None})['list']
    marker='webdock:submission:'+profile['customerID']
    for row in accounts:
        if row.get('name')=='wd-submit' and row.get('domainId')==domain['id'] and row.get('description')!=marker:raise ValueError('Submission account ownership conflict')
    account=ensure('Account',accounts,lambda row:row.get('name')=='wd-submit' and row.get('domainId')==domain['id'],{
        '@type':'User','name':'wd-submit','domainId':domain['id'],'description':marker,'roles':{'@type':'User'},
        'permissions':{'@type':'Replace','enabledPermissions':{'authenticate':True,'emailSend':True},'disabledPermissions':{}},
        'credentials':{'0':{'@type':'Password','secret':profile['password']}},'quotas':{'maxDiskQuota':1048576},'encryptionAtRest':{'@type':'Disabled'},'memberGroupIds':{}})
    url='http://mail-gateway.spitzli-mail-gateway.svc.cluster.local:3139/internal/smtp/'+profile['customerID']
    hook=ensure('MtaHook',call('x:MtaHook/get',{'ids':None})['list'],lambda row:row.get('url')==url,{
        'url':url,'enable':{'else':"authenticated_as == '"+profile['username']+"'"},'stages':{'data':True},'tempFailOnError':True,'timeout':10000,'maxResponseSize':4096,'allowInvalidCerts':False,
        'httpAuth':{'@type':'Bearer','bearerToken':{'@type':'Value','secret':profile['hookToken']}},'httpHeaders':{}})
    route=ensure('MtaRoute',call('x:MtaRoute/get',{'ids':None})['list'],lambda row:row.get('name')=='webdock-relay',{
        '@type':'Relay','name':'webdock-relay','address':profile['relayHost'],'port':465,'protocol':'smtp','implicitTls':True,'allowInvalidCerts':False,'authUsername':profile['relayUsername'],'authSecret':{'@type':'Value','secret':profile['relayPassword']}})
    tls=ensure('MtaTlsStrategy',call('x:MtaTlsStrategy/get',{'ids':None})['list'],lambda row:row.get('name')=='webdock-relay',{
        'name':'webdock-relay','dane':'disable','mtaSts':'disable','startTls':'require','allowInvalidCerts':False})
    call('x:MtaStageAuth/set',{'update':{'singleton':{'mustMatchSender':{'else':'true','match':{'0':{'if':"authenticated_as == '"+profile['username']+"'",'then':'false'}}}}}})
    call('x:MtaOutboundStrategy/set',{'update':{'singleton':{'route':{'match':{'0':{'if':'is_local_domain(rcpt_domain)','then':"'local'"}},'else':"'webdock-relay'"},'tls':{'else':"'webdock-relay'"}}}})
    settings=call('x:SystemSettings/get',{'ids':['singleton']})['list'][0]
    certificate=settings.get('defaultCertificateId')
    expected_certificate=(profile.get('resourceIDs') or {}).get('certificateID')
    if expected_certificate and certificate!=expected_certificate:raise ValueError('Private certificate ownership changed')
    if not certificate:
        candidates=[row for row in call('x:Certificate/get',{'ids':None})['list'] if profile['hostname'] in row.get('subjectAlternativeNames',{})]
        if len(candidates)>1:raise ValueError('Ambiguous private certificate')
        if candidates:certificate=candidates[0]['id']
        else:
            with tempfile.TemporaryDirectory(prefix='webdock-mail-cert-') as directory:
                private=Path(directory)/'key.pem';public=Path(directory)/'cert.pem'
                subprocess.run(['openssl','req','-x509','-newkey','rsa:2048','-nodes','-days','825','-subj','/CN='+profile['hostname'],'-addext','subjectAltName=DNS:'+profile['hostname'],'-keyout',str(private),'-out',str(public)],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=20)
                certificate=call('x:Certificate/set',{'create':{'resource':{'certificate':{'@type':'Text','value':public.read_text()},'privateKey':{'@type':'Text','secret':private.read_text()}}}})['created']['resource']['id']
        call('x:SystemSettings/set',{'update':{'singleton':{'defaultCertificateId':certificate}}})
    return {'accountID':account,'hookID':hook,'routeID':route,'tlsID':tls,'certificateID':certificate}


def verify_submission_listener(client,profile):
    import hashlib
    from urllib.parse import urlsplit
    fingerprint=client.submission_certificate()
    with smtplib.SMTP_SSL(urlsplit(client.url).hostname,2465,context=ssl._create_unverified_context(),timeout=15) as smtp:
        if hashlib.sha256(smtp.sock.getpeercert(binary_form=True)).hexdigest()!=fingerprint:raise ValueError('Submission certificate changed')
        smtp.ehlo('mail.webdock.dev');smtp.login(profile['username'],profile['password'])
        message=('From: '+profile['username']+'\r\nTo: '+profile['username']+'\r\nSubject: Webdock policy readiness probe\r\nX-Webdock-Submission: 1.'+'p'*43+'\r\n\r\nThis probe must be rejected.\r\n')
        try:smtp.sendmail(profile['username'],[profile['username']],message)
        except smtplib.SMTPDataError as error:
            if error.smtp_code==550 and b'Webdock submission is not authorized.' in error.smtp_error:return fingerprint
            raise ValueError('Submission policy probe was not confirmed') from None
        raise ValueError('Submission policy allowed an unreserved message')


def execute_submission(client,packet,deployment,service,guard):
    import json
    import time
    from executor import apply,get,check_owner,kubectl
    from mail_executor import mail_isolation_policy
    profile=packet['submission'];guard()
    owner={'projectID':packet['customerID'],'appID':packet['customerID'],'clusterID':packet['clusterID'],'revision':int(packet['revision']),'leaseUntil':packet['leaseUntil']}
    addresses=verify_relay(profile);guard();owner['leaseUntil']=packet['leaseUntil']
    policy=submission_network_policy(profile,deployment['metadata']['labels'],addresses)
    apply(mail_isolation_policy(packet['customerID'],packet['clusterID']),owner)
    apply(policy,owner)
    observed=get('networkpolicy',policy['metadata']['name'],policy['metadata']['namespace'])
    if not observed or observed['spec']!=policy['spec']:raise ValueError('Mail egress policy was not confirmed')
    policies=json.loads(kubectl('-n',policy['metadata']['namespace'],'get','networkpolicies','-o','json'))['items']
    allowed={'webdock-deny','webdock-internal','webdock-dns','webdock-ingress','spitzli-mail-gateway','webdock-mail-submission'}
    for item in policies:
        name=item['metadata']['name'];spec=item['spec']
        if name not in allowed:raise ValueError('Unexpected Mail network policy')
        if name in ('webdock-deny','webdock-internal') and spec.get('egress'):raise ValueError('Unrestricted internal Mail egress')
        if name=='webdock-dns':
            expected=[{'to':[{'namespaceSelector':{'matchLabels':{'kubernetes.io/metadata.name':'kube-system'}},'podSelector':{'matchLabels':{'k8s-app':'kube-dns'}}}],'ports':[{'protocol':'UDP','port':53},{'protocol':'TCP','port':53}]}]
            if spec.get('egress')!=expected:raise ValueError('Mail DNS policy changed')
        if name in ('webdock-ingress','spitzli-mail-gateway') and spec.get('egress'):raise ValueError('Unrestricted Mail gateway egress')
    resources=configure_submission(client,profile,guard)
    namespace=service['metadata']['namespace'];name=deployment['metadata']['name']
    current=get('deployment',name,namespace);check_owner(current,owner)
    if current['metadata']['uid']!=deployment['metadata']['uid']:raise ValueError('Mail instance ownership changed')
    guard()
    # A distinct annotation manager leaves replica ownership with the native provisioning controller.
    patch={'metadata':{'resourceVersion':current['metadata']['resourceVersion']},'spec':{'template':{'metadata':{'annotations':{'webdock.dev/submission-config':packet['operationID']},'labels':{'security.webdock.dev/ingress':None}}}}}
    kubectl('-n',namespace,'patch','deployment',name,'--type=merge','--field-manager=webdock-mail-config','-p',json.dumps(patch))
    deadline=time.monotonic()+70
    while True:
        guard();current=get('deployment',name,namespace);check_owner(current,owner)
        if current['metadata']['uid']!=deployment['metadata']['uid']:raise ValueError('Mail deployment changed')
        state=current.get('status',{})
        if state.get('observedGeneration',0)>=current['metadata']['generation'] and state.get('updatedReplicas',0)==1 and state.get('availableReplicas',0)==1:
            client.verify();break
        if time.monotonic()>deadline:raise ValueError('Mail configuration restart timed out')
        time.sleep(.5)
    fingerprint=verify_submission_listener(client,profile)
    # Prove egress isolation against a live, controlled service, not an arbitrary closed Internet port.
    gateway=get('service','mail-gateway','spitzli-mail-gateway')
    if not gateway or gateway['metadata'].get('labels',{}).get('security.webdock.dev/managed-by')!='webdock':raise ValueError('Mail gateway ownership is unavailable')
    address=str(ipaddress.ip_address(gateway['spec']['clusterIP']))
    if not ipaddress.ip_address(address).is_private:raise ValueError('Invalid Mail gateway address')
    def host_check():
        with socket.create_connection((address,3138),timeout=3):pass
    host_check();guard()
    probe="""if timeout 3 bash -c 'exec 3<>/dev/tcp/$1/$2' webdock-probe "$1" "$2" 2>/dev/null; then printf allowed; else printf denied; fi"""
    result=kubectl('-n',namespace,'exec','deployment/'+name,'--','/usr/bin/bash','-c',probe,'webdock-probe',address,'3138',timeout=10).strip()
    host_check()
    if result!='denied':raise ValueError('Mail egress isolation is not enforced')
    return {'ready':True,'resources':resources,'certificateSha256':fingerprint}
