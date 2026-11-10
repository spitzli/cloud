"""Server-side admission probes in an explicitly selected disposable loopback cluster."""
import argparse
import copy
import json
import os
import subprocess


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--kubeconfig',required=True);args=parser.parse_args()
    if os.environ.get('SPITZLI_DISPOSABLE_POSTGRES_TEST')!='yes':raise SystemExit('Explicit disposable test confirmation required')
    k=['kubectl','--kubeconfig',args.kubeconfig]
    conf=json.loads(subprocess.check_output(k+['config','view','--minify','-o','json']))
    if not conf['clusters'][0]['cluster']['server'].startswith('https://127.0.0.1:'):raise SystemExit('Only a loopback test cluster is accepted')
    for name in ['webdock-database-pods','webdock-database-services']:
        policy=json.loads(subprocess.check_output(k+['get','validatingadmissionpolicy',name,'-o','json']))
        assert not policy.get('status',{}).get('typeChecking',{}).get('expressionWarnings'),'Admission CEL type checking failed'
    namespace={'apiVersion':'v1','kind':'Namespace','metadata':{'name':'wd-postgres-admission-probe','labels':{'security.webdock.dev/ready':'true','pod-security.kubernetes.io/enforce':'restricted'}}}
    subprocess.run(k+['apply','-f','-'],input=json.dumps(namespace),text=True,capture_output=True,check=True)
    image='ghcr.io/cloudnative-pg/postgresql:18.6-202610120828-standard-trixie@sha256:c7747d8d1049fff9cf30d9cd83d14d609c957afb5ffbcd114145f393ea1850e1'
    resources={'cpu':'10m','memory':'16777216','ephemeral-storage':'1048576'}
    container={'name':'probe','image':image,'command':['/bin/true'],'securityContext':{'allowPrivilegeEscalation':False,'readOnlyRootFilesystem':True,'capabilities':{'drop':['ALL']}},'resources':{'requests':resources,'limits':resources}}
    for name in ['startupProbe','readinessProbe','livenessProbe']:container[name]={'exec':{'command':['true']}}
    pod={'apiVersion':'v1','kind':'Pod','metadata':{'name':'admission-probe','namespace':'wd-db-126'},'spec':{'automountServiceAccountToken':True,'restartPolicy':'Never','securityContext':{'runAsNonRoot':True,'runAsUser':26,'seccompProfile':{'type':'RuntimeDefault'}},'containers':[container]}}
    def admitted(value):
        result=subprocess.run(k+['create','--dry-run=server','-f','-'],input=json.dumps(value),text=True,capture_output=True)
        return result.returncode==0,result.stderr
    allowed,error=admitted(pod);assert allowed,'Valid CNPG-shaped pod rejected: '+error[-500:]
    app=copy.deepcopy(pod);app['metadata']['namespace']='wd-postgres-admission-probe'
    allowed,error=admitted(app);assert not allowed and 'token' in error.lower(),'App API-token exception leaked'
    foreign=copy.deepcopy(pod);foreign['spec']['volumes']=[{'name':'foreign','persistentVolumeClaim':{'claimName':'foreign-claim'}}]
    allowed,error=admitted(foreign);assert not allowed and 'PVC' in error,'Foreign PVC was not rejected by the DB boundary'
    image_pod=copy.deepcopy(pod);image_pod['spec']['containers'][0]['image']='busybox:latest'
    allowed,error=admitted(image_pod);assert not allowed and 'immutable' in error,'Unapproved DB image was accepted'
    service={'apiVersion':'v1','kind':'Service','metadata':{'name':'admission-probe','namespace':'wd-db-126','labels':{'webdock.dev/database-installation':'126'}},'spec':{'type':'NodePort','ports':[{'port':5432,'targetPort':5432,'nodePort':31999,'protocol':'TCP'}]}}
    allowed,error=admitted(service);assert not allowed and 'reserved port' in error,'Out-of-range NodePort accepted'
    print(json.dumps({'validDatabasePod':True,'ordinaryAppTokenDenied':True,'foreignPVCDenied':True,'unapprovedImageDenied':True,'invalidNodePortDenied':True,'typeCheckingWarnings':0}))


if __name__=='__main__':main()
