"""Offline Helm boundaries. No cluster, Secret values, Docker pull or apply."""
import ast,copy,json,os,shutil,subprocess,tempfile,unittest
from pathlib import Path
import yaml
ROOT=Path(__file__).resolve().parents[2]
CHART=ROOT/'deploy/helm/webdock'
BASE={'image':'registry.example/webdock/backend@sha256:'+'a'*64,'existingSecret':'owned-core-env','apiHost':'api.example'}
def configured():
    result=copy.deepcopy(BASE)
    result['buildController']={'enabled':True,'namespace':'owned-controller','namespaceProfile':'trusted-controller-root-tmpfs-v1','existingPrivatePVC':'owned-controller-private','credentialConfigExistingSecret':'owned-controller-bootstrap','privateControlURL':'https://private-core.example/api/hosting/git/worker','privateCoreTLSExistingService':'owned-private-core-tls','privateCoreTLSNamespace':'owned-core','network':{'dnsIPv4':['10.43.0.10'],'coreHTTPSIPv4':['10.43.1.2'],'buildAPIIPv4':['10.83.0.1'],'registryHTTPSIPv4':['203.0.113.1'],'stagingHTTPSIPv4':['203.0.113.2']}}
    return result
@unittest.skipUnless(shutil.which('helm'),'helm required')
class BuildControllerChart(unittest.TestCase):
    def render(self,values,success=True):
        with tempfile.TemporaryDirectory() as root:
            path=Path(root)/'values.json';path.write_text(json.dumps(values))
            result=subprocess.run(['helm','template','owned',str(CHART),'--namespace','owned-core','-f',str(path)],capture_output=True,text=True)
        self.assertEqual(result.returncode==0,success,result.stderr)
        return list(yaml.safe_load_all(result.stdout)) if success else result.stderr
    def test_default_disabled_emits_only_existing_core(self):
        docs=self.render(BASE)
        self.assertEqual([(d['kind'],d['metadata']['name']) for d in docs],[('Service','owned-core'),('Deployment','owned-core')])
    def test_enabled_requires_private_refs_namespace_profile_and_network(self):
        for key in ['existingPrivatePVC','credentialConfigExistingSecret','namespace','namespaceProfile','privateControlURL','privateCoreTLSExistingService','privateCoreTLSNamespace']:
            with self.subTest(key=key):
                c=configured();c['buildController'][key]='';self.render(c,False)
        for namespace in ['owned-core','default','kube-system','webdock-build-qualification']:
            c=configured();c['buildController']['namespace']=namespace;self.render(c,False)
        c=configured();c['buildController']['network']['buildAPIIPv4']=[];self.render(c,False)
    def test_false_activation_bad_pin_urls_endpoints_and_privilege_overrides_rejected(self):
        cases=[('enabled','true'),('enabled',1),('image','registry.example/image:latest'),('privateControlURL','http://private-core.example/api/hosting/git/worker'),('privateControlURL','https://user:password@example/api/hosting/git/worker'),('privateControlURL','https://example/__workspace/api/hosting/git/worker'),('privateControlURL','https://example/api/hosting/git/worker\nX-Test: bad'),('securityContext',{'privileged':True}),('hostNetwork',True),('resources',{'limits':{'memory':'1Ti'}})]
        for key,value in cases:
            with self.subTest(key=key,value=value):
                c=configured();(c if key=='image' else c['buildController'])[key]=value;self.render(c,False)
        for ip in ['0.0.0.0','10.0.0.0/8','127.0.0.1','169.254.169.254','999.0.0.1','example.com','10.0.0.1\nX:evil']:
            c=configured();c['buildController']['network']['coreHTTPSIPv4']=[ip];self.render(c,False)
    def test_fixed_workload_privileges_memory_mounts_and_bootstrap(self):
        docs=self.render(configured());worker=next(d for d in docs if d['kind']=='Deployment' and d['metadata']['name']=='owned-build-controller')
        self.assertEqual(worker['metadata']['namespace'],'owned-controller');self.assertEqual(worker['spec']['replicas'],1);self.assertEqual(worker['spec']['strategy'],{'type':'Recreate'})
        pod=worker['spec']['template']['spec'];self.assertIs(pod['automountServiceAccountToken'],False)
        self.assertNotIn('hostNetwork',pod);self.assertNotIn('hostPID',pod)
        self.assertEqual(pod['securityContext']['runAsUser'],65532)
        main=pod['containers'][0];self.assertEqual(main['resources']['requests']['memory'],'6Gi');self.assertEqual(main['resources']['limits']['memory'],'6Gi');self.assertEqual(main['resources']['limits']['cpu'],'2')
        self.assertNotIn('envFrom',main);self.assertNotIn('swap',json.dumps(pod).lower())
        self.assertEqual(main['command'],['/usr/local/bin/webdock-kubernetes-build-worker'])
        for c in pod['containers']+pod['initContainers']:
            self.assertFalse(c['securityContext']['allowPrivilegeEscalation']);self.assertTrue(c['securityContext']['readOnlyRootFilesystem']);self.assertEqual(c['securityContext']['capabilities']['drop'],['ALL'])
        root=pod['initContainers'][0];self.assertEqual(root['securityContext']['capabilities']['add'],['CHOWN']);self.assertEqual(root['securityContext']['runAsUser'],0)
        self.assertEqual(root['volumeMounts'],[{'name':'upload','mountPath':'/upload'}]);self.assertNotIn('env',root)
        proof=ast.parse((CHART/'tests/qualify-upload-root.py').read_text())
        qualified_command=next(node.value.value for node in ast.walk(proof) if isinstance(node,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='command' for t in node.targets))
        self.assertEqual(root['command'][2],qualified_command)
        volumes={v['name']:v for v in pod['volumes']};self.assertEqual(volumes['upload']['emptyDir'],{'medium':'Memory','sizeLimit':'1Gi'});self.assertEqual(volumes['private']['persistentVolumeClaim']['claimName'],'owned-controller-private')
        self.assertFalse(any('subPath' in m for m in main['volumeMounts']))
        bootstrap=pod['initContainers'][1]['command'][2]
        preserve=bootstrap.split('if [ -e /private/identity/token ]',1)[1].split('else',1)[0]
        self.assertIn('check_file /private/identity/token',preserve);self.assertNotIn('cat ',preserve);self.assertNotIn('mv ',preserve)
        self.assertIn('ln "$temporary" /private/identity/token',bootstrap)
        self.assertFalse(any(d['kind'] in ['Namespace','PersistentVolumeClaim','Secret','RoleBinding','ClusterRoleBinding','Ingress'] for d in docs))
        policy=next(d for d in docs if d['kind']=='ValidatingAdmissionPolicy')
        self.assertEqual(policy['spec']['failurePolicy'],'Fail');self.assertEqual(policy['spec']['matchConditions'][0]['expression'],'request.namespace == "owned-controller"')
        validations='\n'.join(v['expression'] for v in policy['spec']['validations'])
        for fragment in ['size(object.spec.initContainers) == 2',"== ['CHOWN']",'!has(object.spec.initContainers[0].volumeMounts[0].subPath)',"quantity('1Gi')",'!object.spec.automountServiceAccountToken','!has(object.spec.initContainers[0].securityContext.privileged)']:
            self.assertIn(fragment,validations)
    def test_real_bootstrap_preserves_renewed_token_and_rejects_unsafe_existing_file(self):
        docs=self.render(configured())
        deployment=next(d for d in docs if d['kind']=='Deployment' and d['metadata']['name']=='owned-build-controller')
        script=deployment['spec']['template']['spec']['initContainers'][1]['command'][2]
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);private=root/'private';private.mkdir();bootstrap=root/'bootstrap';bootstrap.mkdir()
            for name,content in [('worker.json','{"enabled":false}'),('bootstrap-token','fresh-private-bootstrap'),('worker-token','private-worker-token')]:
                (bootstrap/name).write_text(content)
            # Same exact filesystem procedure, mapped to an owned test root and effective test UID.
            local=script.replace('/private/',str(private)+'/').replace('/bootstrap/',str(bootstrap)+'/').replace('65532',str(os.getuid()))
            run=lambda:subprocess.run(['/bin/sh','-ec',local],capture_output=True,text=True)
            result=run();self.assertEqual(result.returncode,0,result.stderr)
            token=private/'identity/token';self.assertEqual(token.read_text(),'fresh-private-bootstrap')
            self.assertEqual(token.stat().st_mode & 0o777,0o600);self.assertEqual(token.parent.stat().st_mode & 0o777,0o700)
            token.write_text('already-renewed-private-token');before=token.stat().st_ino
            (bootstrap/'bootstrap-token').write_text('stale-bootstrap-must-never-overwrite')
            result=run();self.assertEqual(result.returncode,0,result.stderr)
            self.assertEqual(token.read_text(),'already-renewed-private-token');self.assertEqual(token.stat().st_ino,before)
            token.chmod(0o640);self.assertNotEqual(run().returncode,0);token.chmod(0o600)
            link=token.parent/'extra-link';os.link(token,link);self.assertNotEqual(run().returncode,0);link.unlink()
            token.unlink();token.symlink_to(bootstrap/'bootstrap-token');self.assertNotEqual(run().returncode,0)
    @unittest.skipUnless(os.environ.get('WEBDOCK_CADDY_BINARY'),'explicit qualified local Caddy binary required')
    def test_actual_caddy_adapts_only_put_loopback_and_no_admin(self):
        c=configured();c['buildController']['tlsUpload']={'enabled':True,'image':'docker.io/library/caddy@sha256:'+'b'*64,'existingTLSSecret':'owned-upload-tls','host':'upload.example','sourceIPv4CIDRs':['10.84.0.0/16']}
        docs=self.render(c);config=next(d for d in docs if d['kind']=='ConfigMap')['data']['Caddyfile']
        with tempfile.TemporaryDirectory() as root:
            path=Path(root)/'Caddyfile';path.write_text(config)
            result=subprocess.run([os.environ['WEBDOCK_CADDY_BINARY'],'adapt','--config',str(path),'--adapter','caddyfile'],capture_output=True,text=True)
        self.assertEqual(result.returncode,0,result.stderr)
        adapted=json.loads(result.stdout);self.assertTrue(adapted['admin']['disabled'])
        self.assertEqual(set(adapted['apps']),{'http','tls'})
        servers=adapted['apps']['http']['servers'];self.assertEqual(len(servers),1)
        server=next(iter(servers.values()));self.assertEqual(server['listen'],[':8443'])
        self.assertIn('127.0.0.1:19445',result.stdout);self.assertNotIn('3130',result.stdout)
    def test_egress_is_default_deny_and_exact_operator_endpoints(self):
        docs=self.render(configured());net=next(d['spec'] for d in docs if d['kind']=='NetworkPolicy')
        self.assertEqual(net['podSelector'],{});self.assertEqual(net['policyTypes'],['Ingress','Egress']);self.assertEqual(net['ingress'],[])
        self.assertEqual(len(net['egress']),5)
        for rule in net['egress']:
            for peer in rule['to']:self.assertTrue(peer['ipBlock']['cidr'].endswith('/32'))
            self.assertTrue(all(p['port'] in [53,443,6443] for p in rule['ports']))
    def test_optional_upload_is_private_fixed_put_only_and_requires_pins(self):
        c=configured();c['buildController']['tlsUpload']={'enabled':True,'image':'docker.io/library/caddy@sha256:'+'b'*64,'existingTLSSecret':'owned-upload-tls','host':'upload.example','sourceIPv4CIDRs':['10.84.0.0/16']}
        docs=self.render(c);service=next(d for d in docs if d['kind']=='Service' and d['metadata']['name'].endswith('-upload'));self.assertEqual(service['spec']['type'],'ClusterIP')
        config=next(d for d in docs if d['kind']=='ConfigMap')['data']['Caddyfile'];self.assertIn('not method PUT',config);self.assertIn('reverse_proxy 127.0.0.1:19445',config);self.assertNotIn('3130',config);self.assertIn('admin off',config)
        for key,value in [('image','caddy:latest'),('existingTLSSecret',''),('sourceIPv4CIDRs',['0.0.0.0/0']),('host','example\nreverse_proxy core:3130')]:
            invalid=copy.deepcopy(c);invalid['buildController']['tlsUpload'][key]=value;self.render(invalid,False)
if __name__=='__main__':unittest.main()
