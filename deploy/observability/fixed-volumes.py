#!/usr/bin/env python3
"""Fixed retained OBS filesystems; reuses the qualified backing/UUID primitives."""
import argparse,importlib.util,json,os,shutil,shlex,socket,subprocess,uuid
from pathlib import Path
VOLUMES={'grafana':(2,472,472),'prometheus':(30,1000,2000),'alertmanager':(1,1000,2000),'loki':(20,10001,10001)}
ROOT=Path('/var/lib/webdock-observability')
STATE=Path('/etc/webdock-observability-volumes.json')
def dropin(units):
 names=' '.join(units)
 return '[Unit]\nRequires='+names+'\nAfter='+names+'\nBindsTo='+names+'\n[Service]\nExecStartPre=/usr/local/libexec/webdock-obs-volumes --verify\n'
def resources(name,gib):
 sc='webdock-obs-'+name+'-v1'
 return [{'apiVersion':'storage.k8s.io/v1','kind':'StorageClass','metadata':{'name':sc},'provisioner':'kubernetes.io/no-provisioner','volumeBindingMode':'WaitForFirstConsumer','reclaimPolicy':'Retain'}, {'apiVersion':'v1','kind':'PersistentVolume','metadata':{'name':sc,'labels':{'webdock.dev/observability-volume':name}},'spec':{'capacity':{'storage':str(gib)+'Gi'},'volumeMode':'Filesystem','accessModes':['ReadWriteOnce'],'persistentVolumeReclaimPolicy':'Retain','storageClassName':sc,'local':{'path':str(ROOT/'mounts'/name)},'nodeAffinity':{'required':{'nodeSelectorTerms':[{'matchExpressions':[{'key':'kubernetes.io/hostname','operator':'In','values':['webdock-obs-eu-01']}]}]}}}}]
def primitives():
 p=Path('/usr/local/libexec/webdock-obs-filesystem-primitives.py')
 if not p.exists():p=Path(__file__).resolve().parents[1]/'build-cluster/bounded-logs.py'
 s=importlib.util.spec_from_file_location('obs_fs_primitives',p);m=importlib.util.module_from_spec(s);s.loader.exec_module(m);return m

def mount_unit(name,record):
 return '[Unit]\nDescription=Fixed retained Webdock OBS '+name+'\nBefore=k3s.service\n[Mount]\nWhat='+record['image']+'\nWhere='+record['target']+'\nType=ext4\nOptions=loop,nodev,nosuid,noexec\n[Install]\nWantedBy=multi-user.target\n'
def dependency_names(text):
 return set(shlex.split(text))
def verify_dependencies(m,state):
 units=[]
 for name,record in state['volumes'].items():
  unit=m.run('/usr/bin/systemd-escape','--path','--suffix=mount',record['target']).stdout.strip();units.append(unit)
  if Path('/etc/systemd/system',unit).read_text()!=mount_unit(name,record):raise RuntimeError('OBS mount unit drift')
  if m.run('/usr/bin/systemctl','is-enabled',unit).stdout.strip()!='enabled':raise RuntimeError('OBS mount not enabled')
 if Path('/etc/systemd/system/k3s.service.d/40-webdock-obs-volumes.conf').read_text()!=dropin(units):raise RuntimeError('K3s storage guard drift')
 for prop in ['Requires','After','BindsTo']:
  actual=dependency_names(m.run('/usr/bin/systemctl','show','k3s','--property='+prop,'--value').stdout)
  if not set(units)<=actual:raise RuntimeError('K3s mount lifetime dependency missing: '+prop)
def verify(m,require_ready=True):
 m.private(STATE);state=json.loads(STATE.read_text())
 if require_ready and state.get('phase')!='ready':raise RuntimeError('OBS volume finalization incomplete; retain and reconcile existing state')
 if set(state['volumes'])!=set(VOLUMES):raise RuntimeError('Volume inventory differs')
 for name,record in state['volumes'].items():
  gib,uid,gid=VOLUMES[name]
  if record['bytes']!=gib*1024**3 or record['target']!=str(ROOT/'mounts'/name) or record['image']!=str(ROOT/'images'/(name+'.ext4')):raise RuntimeError('Fixed profile drift')
  m.verify(record);info=Path(record['target']).stat()
  if info.st_uid!=uid or info.st_gid!=gid or info.st_mode&0o7777!=0o2770:raise RuntimeError('Volume root owner/mode drift')
 if require_ready:verify_dependencies(m,state)
 return state

def install(m):
 if STATE.exists():verify(m);print('Existing OBS filesystem identities verified');return
 pods=json.loads(m.run('/usr/local/bin/k3s','kubectl','get','pods','-A','-o','json').stdout)['items']
 if any(p['metadata']['namespace']!='kube-system' for p in pods):raise RuntimeError('Refuse initial storage setup with non-system Pods')
 if ROOT.exists():raise RuntimeError('Refuse existing unowned OBS storage root')
 free=os.statvfs('/var/lib');needed=(sum(x[0] for x in VOLUMES.values())+48)*1024**3
 if free.f_bavail*free.f_frsize<needed:raise RuntimeError('Insufficient hard filesystem plus host reserve')
 ROOT.mkdir(mode=0o700);(ROOT/'images').mkdir(mode=0o700);(ROOT/'mounts').mkdir(mode=0o700)
 state={'owner':'webdock-obs-eu-01','phase':'initializing','volumes':{}}
 # Journal every new backing inode before formatting; partial failure stays closed.
 m.save_record(STATE,state)
 units=[]
 for name,(gib,uid,gid) in VOLUMES.items():
  image=ROOT/'images'/(name+'.ext4');target=ROOT/'mounts'/name;target.mkdir(mode=0o700)
  fd=os.open(image,os.O_RDWR|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
  try:os.posix_fallocate(fd,0,gib*1024**3);os.fsync(fd)
  finally:os.close(fd)
  info=image.stat();record={'state':'initializing','image':str(image),'target':str(target),'uuid':str(uuid.uuid4()),'bytes':gib*1024**3,'inode':info.st_ino,'device':info.st_dev}
  state['volumes'][name]=record;m.save_record(STATE,state)
  m.run('/usr/sbin/mkfs.ext4','-q','-m','0','-U',record['uuid'],'-E','nodiscard,lazy_itable_init=0,lazy_journal_init=0',str(image))
  m.run('/usr/bin/chattr','+i',str(target))
  m.run('/usr/bin/mount','-t','ext4','-o','loop,nodev,nosuid,noexec',str(image),str(target))
  os.chown(target,uid,gid);target.chmod(0o2770)
  record['state']='ready';m.save_record(STATE,state);m.verify(record)
  unit=m.run('/usr/bin/systemd-escape','--path','--suffix=mount',str(target)).stdout.strip();units.append(unit)
  Path('/etc/systemd/system',unit).write_text(mount_unit(name,record))
 # Verify actual noexec identity and ownership before binding K3s lifetime to mounts.
 verify(m,require_ready=False)
 installed=Path('/usr/local/libexec/webdock-obs-volumes');shutil.copyfile(Path(__file__).resolve(),installed);installed.chmod(0o755)
 directory=Path('/etc/systemd/system/k3s.service.d');directory.mkdir(mode=0o755,exist_ok=True)
 (directory/'40-webdock-obs-volumes.conf').write_text(dropin(units))
 m.run('/usr/bin/systemctl','daemon-reload')
 for unit in units:m.run('/usr/bin/systemctl','enable','--now',unit)
 verify_dependencies(m,state)
 state['phase']='ready';m.save_record(STATE,state);verify(m)
 print(json.dumps({'status':'installed','volumes':{n:{'bytes':r['bytes'],'uuid':r['uuid']} for n,r in state['volumes'].items()},'hostFallback':'immutable unmounted directories plus K3s mount dependencies'}))
def main():
 p=argparse.ArgumentParser(description=__doc__);g=p.add_mutually_exclusive_group(required=True);g.add_argument('--install',action='store_true');g.add_argument('--verify',action='store_true');g.add_argument('--resources',action='store_true');a=p.parse_args()
 if a.resources:
  print(json.dumps({'apiVersion':'v1','kind':'List','items':[x for n,(size,_,_) in VOLUMES.items() for x in resources(n,size)]}));return
 if os.geteuid()!=0 or socket.gethostname()!='webdock-obs-eu-01':raise SystemExit('Only root on exact owned OBS host')
 m=primitives()
 if a.install:install(m)
 else:verify(m);print('All OBS backing inodes, UUIDs, limits, loop devices and owners verified')
if __name__=='__main__':main()
