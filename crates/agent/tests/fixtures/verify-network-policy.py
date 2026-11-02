#!/usr/bin/env python3
"""Actual chart traffic tests, only in an explicitly owned disposable Kind cluster."""
import argparse,json,pathlib,secrets,subprocess,tempfile,time,yaml
parser=argparse.ArgumentParser();parser.add_argument('--fixture',required=True);args=parser.parse_args()
m=json.loads(pathlib.Path(args.fixture).read_text());assert m['name'].startswith('webdock-rust-native-') and m['context']=='kind-'+m['name']
node=json.loads(subprocess.check_output(['docker','inspect',m['node']]))[0];assert node['Config']['Labels'].get('io.x-k8s.kind.cluster')==m['name']
cmd=['kubectl','--kubeconfig',m['kubeconfig'],'--context',m['context']]
def call(argv,source=None,check=True,timeout=40):
 r=subprocess.run(cmd+argv,input=source,capture_output=True,text=True,timeout=timeout)
 if check and r.returncode:raise RuntimeError(r.stderr[-3000:])
 return r
suffix=secrets.token_hex(4);runtime='wd-policy-'+suffix;control='wd-control-'+suffix;other='wd-other-'+suffix
image='docker.io/library/busybox@sha256:bdf57e528e45e4433820e045b29b4597825a1c9e38353532d90a01445013f82e'
labels={'webdock.dev/installation':'test-policy','webdock.dev/project-id':'123','webdock.dev/resource-id':'456','webdock.dev/managed-by':'webdock'}
owned=[]
def apply(value):call(['apply','-f','-'],json.dumps(value))
def pod(namespace,name,component):
 return {'apiVersion':'v1','kind':'Pod','metadata':{'namespace':namespace,'name':name,'labels':{'webdock.dev/component':component}},'spec':{'automountServiceAccountToken':False,'securityContext':{'runAsNonRoot':True,'runAsUser':65532,'seccompProfile':{'type':'RuntimeDefault'}},'containers':[{'name':'probe','image':image,'command':['sh','-c','echo fixture >/tmp/index.html; httpd -p 443 -h /tmp; exec httpd -f -p 8080 -h /tmp'],'resources':{'limits':{'cpu':'100m','memory':'33554432'},'requests':{'cpu':'10m','memory':'16777216'}},'securityContext':{'allowPrivilegeEscalation':False,'readOnlyRootFilesystem':True,'capabilities':{'drop':['ALL'],'add':['NET_BIND_SERVICE']}},'volumeMounts':[{'name':'tmp','mountPath':'/tmp'}]}],'volumes':[{'name':'tmp','emptyDir':{'medium':'Memory','sizeLimit':'16777216'}}]}}
def traffic(namespace,podname,address,allowed):
 result=call(['exec','-n',namespace,podname,'--','wget','-T','3','-qO-',address],check=False,timeout=10)
 assert (result.returncode==0)==allowed, f'traffic expected allowed={allowed}: {namespace} -> {address}; exit={result.returncode}'
try:
 for namespace,extra in [(runtime,{}),(control,{'webdock.dev/role':'control-plane'}),(other,{})]:
  apply({'apiVersion':'v1','kind':'Namespace','metadata':{'name':namespace,'labels':{'dev.webdock.test':'runtime-policy-fixture','pod-security.kubernetes.io/enforce':'restricted',**extra}}});owned.append(namespace)
 apply(pod(control,'control','control-plane'));apply(pod(other,'outside','untrusted'))
 values={'name':'wd-test-policy-456','image':image,'labels':labels,'revision':1,'generation':1,'specHash':'a'*64,'arguments':['sh','-c','echo fixture >/tmp/index.html; exec httpd -f -p 8080 -h /tmp'],'port':8080,'healthPath':'/'}
 with tempfile.NamedTemporaryFile(mode='w',suffix='.json') as file:
  json.dump(values,file);file.flush();rendered=subprocess.check_output(['helm','template','runtime','deploy/helm/webdock-plugin','--namespace',runtime,'--values',file.name],text=True)
 objects=list(yaml.safe_load_all(rendered));assert len([o for o in objects if o and o.get('kind')=='NetworkPolicy'])==1
 call(['apply','-n',runtime,'-f','-'],rendered)
 for namespace in owned:call(['wait','pod','--all','--for=condition=Ready','-n',namespace,'--timeout=120s'],timeout=125)
 def getpods(namespace):return json.loads(call(['get','pods','-n',namespace,'-o','json']).stdout)['items']
 rpod=getpods(runtime)[0];cp=getpods(control)[0];op=getpods(other)[0]
 rname=rpod['metadata']['name'];rip=rpod['status']['podIP'];cip=cp['status']['podIP'];oip=op['status']['podIP']
 # Positive controls establish both destination listeners before checking denied paths.
 traffic(other,'outside',f'http://{cip}:8080/',True)
 traffic(control,'control',f'http://{rip}:8080/',True)
 traffic(runtime,rname,f'http://{cip}:443/',True)
 traffic(other,'outside',f'http://{rip}:8080/',False)
 traffic(runtime,rname,f'http://{cip}:8080/',False)
 traffic(runtime,rname,f'http://{oip}:8080/',False)
 call(['exec','-n',runtime,rname,'--','nslookup','kubernetes.default.svc.cluster.local'],timeout=10)
 print('PASS actual chart NetworkPolicy: control ingress/443 egress and DNS allowed; foreign ingress, wrong port and foreign egress denied')
finally:
 for namespace in owned:call(['delete','namespace',namespace,'--wait=false'],check=False)
