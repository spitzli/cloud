#!/usr/bin/env python3
"""Offline review renderer and private-input guard. Never provisions a host.

Credential renewal deployment is blocked pending real host/RBAC qualification.
The worker's managed file path is implemented separately; review outputs do not
install it, issue credentials or activate a service.
"""
import argparse
import base64
import copy
import hashlib
import ipaddress
import json
import os
from pathlib import Path
import re
import socket
import ssl
import stat
import sys
import time

ROOT=Path(__file__).resolve().parent
NS='webdock-build-controller'
SA='webdock-build-controller'
REVOCATION='webdock-controller-revocation'
EXECUTION='webdock-build-qualification'
INTERFACE='wd-build-ctl'
API='10.83.0.1'
SUBJECT='system:serviceaccount:'+NS+':'+SA
BLOCKER='Managed token-file renewal is implemented with controlled API tests; real host bindings, pinned-cluster RBAC/admission and revocation/restart qualification remain pending.'


def validate(c):
    expected=set(json.loads((ROOT/'connection.example.json').read_text()))
    if set(c)!=expected or c['version']!=1 or type(c['enabled']) is not bool:raise ValueError('exact versioned controller profile required')
    if c['buildHostName']!='webdock-build-eu-01' or c['buildPublicIPv4']!='167.86.75.144' or c['apiEndpoint']!='https://10.83.0.1:6443':raise ValueError('fixed BUILD endpoint required; no public/local-tunnel API alias')
    if not isinstance(c['controllerHostName'],str) or not re.fullmatch(r'[a-z0-9][a-z0-9.-]{0,62}',c['controllerHostName']) or c['controllerHostName']==c['buildHostName']:raise ValueError('explicit distinct controller host required')
    address=ipaddress.ip_address(c['controllerPublicIPv4'])
    if address.version!=4 or not address.is_global or str(address)=='167.86.75.144':raise ValueError('explicit approved controller public IPv4 outside BUILD required')
    peer=ipaddress.ip_address(c['controllerTunnelIPv4'])
    if peer.version!=4 or peer not in ipaddress.ip_network('10.83.0.0/24') or peer in {ipaddress.ip_address(x) for x in ['10.83.0.0','10.83.0.1','10.83.0.255']}:raise ValueError('one explicit unused controller /32 in the BUILD private range required')
    keys=[]
    for name in ['buildPublicKey','controllerPublicKey']:
        if not isinstance(c[name],str):raise ValueError('operator-supplied WireGuard public keys required')
        try:key=base64.b64decode(c[name],validate=True)
        except Exception:raise ValueError('invalid WireGuard public key') from None
        if len(key)!=32 or key==bytes(32):raise ValueError('invalid WireGuard public key')
        keys.append(key)
    if keys[0]==keys[1]:raise ValueError('distinct peer keys required')
    for name in ['buildPort','controllerPort']:
        if type(c[name]) is not int or not 1024<=c[name]<=65535 or c[name] in {3128,6443,10250,10251,19443,19444}:raise ValueError('dedicated WireGuard UDP ports required')
    for name in ['buildPrivateKeyFile','controllerPrivateKeyFile','buildCAFile','bearerFile','kubeconfigFile']:
        value=c[name]
        if not isinstance(value,str) or not re.fullmatch(r'/(etc|var/lib)/[A-Za-z0-9_./-]+',value) or '..' in Path(value).parts:raise ValueError('absolute owned /etc or /var/lib file references required')
    if len({c[name] for name in ['buildCAFile','bearerFile','kubeconfigFile','controllerPrivateKeyFile']})!=4:raise ValueError('distinct CA, bearer, kubeconfig and private-key files required')
    if not isinstance(c['buildCASHA256'],str) or not re.fullmatch('[a-f0-9]{64}',c['buildCASHA256']):raise ValueError('operator-verified BUILD CA pin required')
    for name in ['serviceAccountUID','revocationSecretUID']:
        if not isinstance(c[name],str) or not re.fullmatch('[A-Za-z0-9_-]{1,128}',c[name]):raise ValueError('actual server-assigned identity/revocation UID required')
    if not isinstance(c['audience'],str) or not re.fullmatch(r'https://[a-z0-9.-]{1,200}(?:/[a-z0-9/_-]*)?',c['audience']):raise ValueError('one explicit verified API audience required')
    return c


def fingerprint(c):return hashlib.sha256(json.dumps(c,sort_keys=True,separators=(',',':')).encode()).hexdigest()


def private_read(path,limit=65536):
    path=Path(path)
    if not path.is_absolute() or any(parent.is_symlink() for parent in [*path.parents,path]):raise ValueError('absolute nonlinked private input required')
    parent=path.parent.stat()
    if not stat.S_ISDIR(parent.st_mode) or parent.st_uid!=os.geteuid() or parent.st_mode&0o077:raise ValueError('owned0700 immediate parent required')
    fd=os.open(path,os.O_RDONLY|os.O_NOFOLLOW)
    with os.fdopen(fd,'rb') as stream:
        info=os.fstat(stream.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_uid!=os.geteuid() or info.st_nlink!=1 or info.st_mode&0o077 or info.st_size>limit:raise ValueError('owned private regular file required')
        return stream.read(limit+1)


def check_claims(claims,c,now,minimum=300):
    # This is a claim/lifetime guard, not a JWT signature verifier. Only accept
    # tokens from the CA-pinned API; the API authenticates their signature on use.
    validate(c)
    for name in ['iat','nbf','exp']:
        if type(claims.get(name)) is not int:raise ValueError('finite integer token times required')
    if claims['iat']>now+30 or claims['nbf']>now+30 or not 0<claims['exp']-claims['iat']<=3600 or claims['exp']-now<minimum or claims['exp']>now+3630:raise ValueError('token expired, near expiry, future or overlong')
    if claims.get('sub')!=SUBJECT or claims.get('aud')!=[c['audience']]:raise ValueError('wrong token subject/audience')
    private=claims.get('kubernetes.io',{})
    if (private.get('namespace')!=NS or private.get('serviceaccount')!={'name':SA,'uid':c['serviceAccountUID']} or private.get('secret')!={'name':REVOCATION,'uid':c['revocationSecretUID']} or 'pod' in private or 'node' in private):raise ValueError('token identity/revocation binding differs')
    return {'subject':SUBJECT,'expiresAt':claims['exp'],'remainingSeconds':claims['exp']-now}


def identity_documents(c):
    subject={'kind':'ServiceAccount','name':SA,'namespace':NS}
    items=[{'apiVersion':'v1','kind':'Namespace','metadata':{'name':NS,'labels':{'webdock.dev/owner':'managed-build-controller','pod-security.kubernetes.io/enforce':'restricted'}}},
        {'apiVersion':'v1','kind':'ResourceQuota','metadata':{'name':'identity-only','namespace':NS},'spec':{'hard':{'pods':'0','secrets':'1','configmaps':'0','persistentvolumeclaims':'0','services':'0'}}},
        {'apiVersion':'v1','kind':'ServiceAccount','metadata':{'name':SA,'namespace':NS},'automountServiceAccountToken':False},
        {'apiVersion':'v1','kind':'Secret','metadata':{'name':REVOCATION,'namespace':NS},'type':'Opaque','immutable':True,'stringData':{'revocation-marker':'owned-controller-v1'}},
        {'apiVersion':'rbac.authorization.k8s.io/v1','kind':'Role','metadata':{'name':'renew-own-controller-token','namespace':NS},'rules':[{'apiGroups':[''],'resources':['serviceaccounts/token'],'resourceNames':[SA],'verbs':['create']}]}]
    for namespace,role in [(NS,'renew-own-controller-token'),(EXECUTION,'webdock-build-dispatcher'),(EXECUTION,'build-volume-observer'),('webdock-build-storage-requests','build-volume-producer'),('webdock-build-storage-receipts','build-volume-producer')]:
        items.append({'apiVersion':'rbac.authorization.k8s.io/v1','kind':'RoleBinding','metadata':{'name':'managed-controller-'+role,'namespace':namespace},'subjects':[subject],'roleRef':{'apiGroup':'rbac.authorization.k8s.io','kind':'Role','name':role}})
    conditions=["request.namespace == "+json.dumps(NS),"request.name == "+json.dumps(SA),"has(object.spec.expirationSeconds) && object.spec.expirationSeconds >= 600 && object.spec.expirationSeconds <= 3600","object.spec.audiences == "+json.dumps([c['audience']]),"has(object.spec.boundObjectRef) && object.spec.boundObjectRef.kind == 'Secret' && object.spec.boundObjectRef.apiVersion == 'v1' && object.spec.boundObjectRef.name == "+json.dumps(REVOCATION)+" && object.spec.boundObjectRef.uid == "+json.dumps(c['revocationSecretUID'])]
    items += [{'apiVersion':'admissionregistration.k8s.io/v1','kind':'ValidatingAdmissionPolicy','metadata':{'name':'webdock-controller-token-request'},'spec':{'failurePolicy':'Fail','matchConstraints':{'resourceRules':[{'apiGroups':[''],'apiVersions':['v1'],'operations':['CREATE'],'resources':['serviceaccounts/token']}]},'matchConditions':[{'name':'controller-identity','expression':'request.userInfo.username == '+json.dumps(SUBJECT)}],'validations':[{'expression':' && '.join('('+value+')' for value in conditions),'message':'Only the exact short-lived controller/revocation-bound API token is permitted.'}]}}, {'apiVersion':'admissionregistration.k8s.io/v1','kind':'ValidatingAdmissionPolicyBinding','metadata':{'name':'webdock-controller-token-request'},'spec':{'policyName':'webdock-controller-token-request','validationActions':['Deny']}}]
    request={'apiVersion':'authentication.k8s.io/v1','kind':'TokenRequest','spec':{'audiences':[c['audience']],'expirationSeconds':3600,'boundObjectRef':{'kind':'Secret','apiVersion':'v1','name':REVOCATION,'uid':c['revocationSecretUID']}}}
    return {'apiVersion':'v1','kind':'List','items':items},request


def render(c):
    validate(c);files={}
    for role in ['build','controller']:
        other='controller' if role=='build' else 'build'
        local_ip=API if role=='build' else c['controllerTunnelIPv4']
        peer_ip=c['controllerTunnelIPv4'] if role=='build' else API
        replacements={'ROLE':role,'BUILD_DEPENDENCY':'webdock-build-network.service' if role=='build' else '', 'LOCAL_PORT':str(c[role+'Port']),'PRIVATE_FILE':c[role+'PrivateKeyFile'],'PEER_KEY':c[other+'PublicKey'],'ALLOWED':peer_ip+'/32','PEER_PUBLIC':c[other+'PublicIPv4'],'PEER_PORT':str(c[other+'Port']),'LOCAL_IP':local_ip,'ADDRESS':'ExecStart=/usr/sbin/ip address add '+local_ip+'/32 dev '+INTERFACE if role=='controller' else ''}
        for source,name in [('link.service.in',role+'-link.service'),('guard.service.in',role+'-guard.service')]:
            text=(ROOT/source).read_text()
            for key,value in replacements.items():text=text.replace('@'+key+'@',value)
            files[name]=text
        input_rules=('    iifname "'+INTERFACE+'" ip saddr '+peer_ip+' ip daddr '+local_ip+' tcp dport 6443 accept\n' if role=='build' else '    iifname "'+INTERFACE+'" ct state established,related accept\n')+'    iifname "'+INTERFACE+'" counter drop\n'
        input_rules='    meta nfproto ipv6 udp dport '+str(c[role+'Port'])+' counter drop\n    ip daddr '+c[role+'PublicIPv4']+' udp dport '+str(c[role+'Port'])+' ip saddr != '+c[other+'PublicIPv4']+' counter drop\n'+input_rules
        output_rules=('  chain output { type filter hook output priority -30; policy accept; oifname "'+INTERFACE+'" ip daddr '+API+' tcp dport 6443 accept; oifname "'+INTERFACE+'" counter drop; }\n' if role=='controller' else '')
        files[role+'-guard.nft']='add table inet webdock_controller_link\nflush table inet webdock_controller_link\ntable inet webdock_controller_link {\n  chain input { type filter hook input priority -30; policy accept;\n'+input_rules+'  }\n  chain forward { type filter hook forward priority -30; policy accept; iifname "'+INTERFACE+'" counter drop; oifname "'+INTERFACE+'" counter drop; }\n'+output_rules+'}\n'
    files['build-host-input.review.nft']='# Review/integrate into the canonical owned BUILD host profile; never run on a RUN node.\ninsert rule inet webdock_build_host input ip saddr '+c['controllerPublicIPv4']+' ip daddr 167.86.75.144 udp dport '+str(c['buildPort'])+' accept comment "webdock-controller-wg-v1"\ninsert rule inet webdock_build_host input iifname "'+INTERFACE+'" ip saddr '+c['controllerTunnelIPv4']+' ip daddr '+API+' tcp dport 6443 accept comment "webdock-controller-api-v1"\n'
    files['controller-host-input.review.nft']='# Rule fragment only: integrate into the approved controller host INPUT chain, whose ownership is still unknown.\nip saddr 167.86.75.144 ip daddr '+c['controllerPublicIPv4']+' udp dport '+str(c['controllerPort'])+' accept\n'
    identity,request=identity_documents(c)
    files['identity-rbac.review.json']=json.dumps(identity,indent=2)+'\n';files['token-request.review.json']=json.dumps(request,indent=2)+'\n'
    canary=copy.deepcopy(request);canary['spec']['expirationSeconds']=600;canary['spec']['audiences']=['https://'+fingerprint(c)[:32]+'.controller-canary.invalid']
    gate={'purpose':'BUILD-local operator activation barrier only; not a worker bearer or deployment action','caller':{'username':SUBJECT,'uid':c['serviceAccountUID']},'endpoint':c['apiEndpoint']+'/api/v1/namespaces/'+NS+'/serviceaccounts/'+SA+'/token','request':canary,'requiredDenial':{'statuses':[403,422],'policy':'webdock-controller-token-request','binding':'webdock-controller-token-request','messageMustIdentifyOwnedPolicyAndBinding':True},'mustPassBeforeBootstrapDelivery':True,'mustPassBeforeWorkerStart':True,'typeCheckingOrPositiveRenewalAloneSufficient':False,'propagation201':'keep any JWT private0600 on BUILD, never deliver/use it, and revoke with the exact owned binding; probe uses600s TTL','cleanup':'retain exact namespace/SA/Secret/policy UIDs and close only owned authority; no global privilege or source namespace changes'}
    files['bootstrap-readiness.review.json']=json.dumps(gate,indent=2)+'\n'
    files['worker-auth.disabled.conf']=(ROOT/'worker-auth.disabled.conf').read_text()
    files['activation.json']=json.dumps({'profileSHA256':fingerprint(c),'networkEnabledInConfig':c['enabled'],'installed':False,'workerActivationBlocked':True,'blocker':BLOCKER},indent=2)+'\n'
    return files


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    mode=parser.add_mutually_exclusive_group(required=True);mode.add_argument('--render',action='store_true');mode.add_argument('--guard',action='store_true');mode.add_argument('--check-token',action='store_true')
    parser.add_argument('--config',type=Path,required=True);parser.add_argument('--role',choices=['build','controller']);args=parser.parse_args()
    try:
        # Offline render accepts an ordinary source config; runtime guards demand private custody.
        c=json.loads(args.config.read_bytes() if args.render else private_read(args.config));validate(c)
        if args.render:print(json.dumps({'files':render(c)},indent=2));return 0
        if args.guard:
            if not c['enabled'] or not args.role or os.geteuid()!=0 or socket.gethostname()!=c[args.role+'HostName']:raise ValueError('disabled or wrong owned root host')
            marker=private_read('/etc/webdock-build-controller/network-approved.sha256',256).decode().strip()
            if marker!=fingerprint(c):raise ValueError('approved network profile differs')
            private_read(c[args.role+'PrivateKeyFile'],256)
            return 0
        ca=private_read(c['buildCAFile'])
        if hashlib.sha256(ca).hexdigest()!=c['buildCASHA256']:raise ValueError('BUILD CA pin differs')
        ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT).load_verify_locations(cadata=ca.decode())
        token=private_read(c['bearerFile'],16384).decode().strip();parts=token.split('.')
        if len(parts)!=3:raise ValueError('invalid token framing')
        claims=json.loads(base64.urlsafe_b64decode(parts[1]+'='*((4-len(parts[1])%4)%4)))
        print(json.dumps(check_claims(claims,c,int(time.time()),minimum=2100)));return 0
    except (ValueError,TypeError,KeyError,OSError,json.JSONDecodeError):
        print('Controller profile/input guard refused; no host changes performed.',file=sys.stderr);return 2


if __name__=='__main__':sys.exit(main())
