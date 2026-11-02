#!/usr/bin/env python3
"""Copy only owned registry fixture data into the explicit disposable Kind node."""
import argparse,json,pathlib,re,shutil,subprocess,tempfile,uuid
p=argparse.ArgumentParser();p.add_argument('--kubeconfig',required=True);p.add_argument('--context',required=True);p.add_argument('--outer-container',required=True);args=p.parse_args()
assert re.fullmatch(r'kind-webdock-rust-[a-z0-9-]+',args.context)
assert re.fullmatch(r'webdock-rust-quota-owned-[a-z0-9-]+',args.outer_container)
engine=['docker','--host','unix:///var/run/docker.sock']
def docker(*words):return subprocess.run(engine+list(words),check=True,capture_output=True)
node=args.context.removeprefix('kind-')+'-control-plane'
node_info=json.loads(docker('inspect',node).stdout)[0];assert node_info['Config']['Labels']['io.x-k8s.kind.cluster']==args.context.removeprefix('kind-')
outer=json.loads(docker('inspect',args.outer_container).stdout)[0];assert outer['Config']['Labels']['dev.webdock.test']=='rust-storage-fixture'
assert 'kind' in node_info['NetworkSettings']['Networks']
attached='kind' not in outer['NetworkSettings']['Networks']
if attached:docker('network','connect','kind',args.outer_container)
outer=json.loads(docker('inspect',args.outer_container).stdout)[0];address=outer['NetworkSettings']['Networks']['kind']['IPAddress']
fixture='/var/local/webdock-rust-tests/registry-'+uuid.uuid4().hex
docker('exec',node,'mkdir','-p',fixture+'/lib');docker('exec',node,'chmod','700',fixture)
config_path=pathlib.Path(args.kubeconfig).resolve(strict=True)
cli=['kubectl','--kubeconfig',str(config_path),'--context',args.context]
config=json.loads(subprocess.run(cli+['config','view','--raw','-o','json'],check=True,capture_output=True).stdout)
for cluster in config['clusters']:cluster['cluster']['server']='https://127.0.0.1:6443'
with tempfile.TemporaryDirectory(prefix='webdock-registry-private-') as temporary:
    state_path=pathlib.Path(temporary)/'state.json';docker('cp',args.outer_container+':/tmp/webdock-registry-fixture.json',str(state_path));state_path.chmod(0o600);state=json.loads(state_path.read_text());state['address']=address;state_path.write_text(json.dumps(state));docker('cp',str(state_path),node+':'+fixture+'/registry-state.json')
    kubeconfig=pathlib.Path(temporary)/'kubeconfig';kubeconfig.write_text(json.dumps(config));kubeconfig.chmod(0o600);docker('cp',str(kubeconfig),node+':'+fixture+'/kubeconfig')
docker('exec',node,'chown','0:0',fixture+'/registry-state.json',fixture+'/kubeconfig')
docker('exec',node,'chmod','600',fixture+'/registry-state.json',fixture+'/kubeconfig')
built=subprocess.run(['cargo','test','-p','webdock-agent','--test','registry_tls','--no-run','--message-format=json'],check=True,capture_output=True,text=True)
executables=[row['executable'] for line in built.stdout.splitlines() if (row:=json.loads(line)).get('reason')=='compiler-artifact' and row.get('target',{}).get('name')=='registry_tls' and row.get('executable')];assert len(executables)==1
for source,target in [(executables[0],'registry-test'),('/usr/local/bin/helm','helm'),('/usr/lib/libc.so.6','lib/libc.so.6'),('/usr/lib/libgcc_s.so.1','lib/libgcc_s.so.1'),('/usr/lib64/ld-linux-x86-64.so.2','lib/ld-linux-x86-64.so.2')]:
    with tempfile.TemporaryDirectory(prefix='webdock-registry-artifact-') as temporary:
        copy=pathlib.Path(temporary)/pathlib.Path(target).name;shutil.copyfile(source,copy);copy.chmod(0o755);docker('cp',str(copy),node+':'+fixture+'/'+target)
docker('cp','deploy/helm/webdock-plugin',node+':'+fixture+'/chart')
result=subprocess.run(engine+['exec','-e','WEBDOCK_DISPOSABLE_NODE_TEST=yes','-e','WEBDOCK_KUBE_TEST_CONFIG='+fixture+'/kubeconfig','-e','WEBDOCK_KUBE_TEST_CONTEXT='+args.context,'-e','WEBDOCK_REGISTRY_FIXTURE_DIR='+fixture,node,fixture+'/lib/ld-linux-x86-64.so.2','--library-path',fixture+'/lib',fixture+'/registry-test','owned_kind','--ignored','--nocapture'])
docker('exec',node,'rm','-r','--',fixture)
if attached:docker('network','disconnect','kind',args.outer_container)
raise SystemExit(result.returncode)
