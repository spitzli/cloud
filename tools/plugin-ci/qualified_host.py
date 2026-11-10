#!/usr/bin/env python3
"""Provision the existing operator-verified EU BUILD host, without tenant changes."""
from __future__ import annotations
import hashlib
import json
import os
from pathlib import Path
import pwd
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
import zipfile

USER='spitzli-plugin-ci'
UID=1001
BASE=Path('/opt/spitzli-plugin-ci')
HOME=Path('/home')/USER
RUNNER=HOME/'actions-runner'
GROUP='cloud-plugin-trusted-eu2'
NAME='cloud-plugin-eu2-trusted-01'
LABELS=['spitzli-plugin-eu2','trusted-platform']
RUNNER_VERSION='2.338.2'
RUNNER_SHA='baebf204307bac6193480852732f0aba2ae97b6c5597c244f4d5558390147c40'
BUN_SHA='3e730528d89775f03f87f05c06787c4df4099d0a37f3f252e7b829cb23816e18'
RUST_IMAGE='docker.io/library/rust:1.98.1-slim-trixie@sha256:4cd829461bd5c4d511c32e269da9cb8929223b666519d8004e35fc8d1d771ab7'
BUILDCTL_SHA='8e64362dceba989827b23af567915feda10426ebd40611772318662e33bf2bfd'
SERVICE=Path('/etc/systemd/system/spitzli-plugin-runner.service')
TOOL_PATH=f'{BASE}/cargo/bin:{BASE}/bun-v1.4.3:{BASE}/buildkit-v0.34.1/bin:/usr/local/bin:/usr/bin:/bin'


def require(condition,message):
    if not condition:raise ValueError(message)


def run(args,**kwargs):
    return subprocess.run(args,check=True,**kwargs)


def safe_member(root,name):
    relative=Path(name)
    require(not relative.is_absolute() and '..' not in relative.parts,'Archive path outside installation')
    target=(root/relative).resolve()
    require(target.is_relative_to(root.resolve()),'Archive path outside installation')
    return target


def download(url,sha,target):
    require(url.startswith('https://github.com/'),'Only official GitHub release downloads are used')
    request=urllib.request.Request(url,headers={'User-Agent':'Spitzli-qualified-CI-setup'})
    digest=hashlib.sha256()
    with urllib.request.urlopen(request,timeout=120) as source,target.open('xb') as output:
        while chunk:=source.read(1024*1024):
            digest.update(chunk);output.write(chunk)
    require(digest.hexdigest()==sha,'Official release checksum mismatch')


def service_unit():
    return f'''[Unit]
Description=Persistent trusted-maintainer Spitzli platform CI (verified Contabo EU2)
After=network-online.target spitzli-plugin-ci-runtime.service spitzli-plugin-ci-egress.service spitzli-plugin-buildkit.service
Wants=network-online.target
Requires=spitzli-plugin-ci-runtime.service spitzli-plugin-ci-egress.service spitzli-plugin-buildkit.service
[Service]
Type=simple
User={USER}
Group={USER}
WorkingDirectory={RUNNER}
Environment=HOME={HOME}
Environment=PATH={TOOL_PATH}
Environment=RUSTUP_HOME={BASE}/rustup
Environment=CARGO_HOME={HOME}/.cargo
Environment=CARGO_BUILD_JOBS=3
Environment=CARGO_TARGET_DIR={HOME}/native-validator-target
Environment=XDG_RUNTIME_DIR=/run/user/{UID}
Environment=SPITZLI_BUILDKIT_ADDR=unix:///run/user/{UID}/spitzli-buildkit.sock
Environment=SPITZLI_BUILDCTL_SHA256={BUILDCTL_SHA}
ExecStart={RUNNER}/bin/runsvc.sh
KillMode=process
KillSignal=SIGTERM
TimeoutStopSec=5min
Restart=on-failure
RestartSec=5
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=no
PrivateTmp=yes
ProtectProc=invisible
ReadWritePaths={HOME} /run/user/{UID}
InaccessiblePaths=-/root -/etc/rancher/k3s -/run/k3s -/var/lib/rancher/k3s
MemoryMax=3G
CPUQuota=200%
TasksMax=512
[Install]
WantedBy=multi-user.target
'''


def install():
    require(os.geteuid()==0,'Host provisioning requires the existing root SSH identity')
    user=pwd.getpwnam(USER)
    require(user.pw_uid==UID and user.pw_dir==str(HOME),'Expected existing qualified BuildKit user required')
    require(hashlib.sha256((BASE/'buildkit-v0.34.1/bin/buildctl').read_bytes()).hexdigest()==BUILDCTL_SHA,'Previously qualified BuildKit client changed')
    run(['systemctl','is-active','--quiet','spitzli-plugin-buildkit.service'])
    environment={**os.environ,'DEBIAN_FRONTEND':'noninteractive','NEEDRESTART_MODE':'l'}
    run(['apt-get','update','-qq'],env=environment)
    run(['apt-get','install','-y','--no-install-recommends','build-essential','pkg-config','libssl-dev','protobuf-compiler','gh','git','ca-certificates','libicu74','libkrb5-3','zlib1g'],env=environment)
    BASE.mkdir(mode=0o755,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='qualified-ci-',dir=BASE) as temporary:
        temporary=Path(temporary)
        if not (BASE/'cargo/bin/rustup').is_file():
            mount=temporary/'rust-image';mount.mkdir()
            run(['/usr/local/bin/k3s','ctr','images','mount',RUST_IMAGE,str(mount)])
            try:
                require((mount/'usr/local/cargo/bin/rustup').is_file(),'Pinned Rust image lacks expected toolchain')
                shutil.copytree(mount/'usr/local/cargo',BASE/'cargo',symlinks=True)
                shutil.copytree(mount/'usr/local/rustup',BASE/'rustup',symlinks=True)
            finally:run(['/usr/local/bin/k3s','ctr','images','unmount',str(mount)])
        bun=BASE/'bun-v1.4.3'
        if not (bun/'bun').is_file():
            archive=temporary/'bun.zip'
            download('https://github.com/oven-sh/bun/releases/download/bun-v1.4.3/bun-linux-x64.zip',BUN_SHA,archive)
            with zipfile.ZipFile(archive) as data:
                for member in data.infolist():safe_member(temporary,member.filename)
                data.extractall(temporary/'bun-extracted')
            bun.mkdir();shutil.copy2(temporary/'bun-extracted/bun-linux-x64/bun',bun/'bun');(bun/'bun').chmod(0o755)
        if not (RUNNER/'bin/Runner.Listener').is_file():
            require(not RUNNER.exists(),'Existing partial runner installation requires review')
            archive=temporary/'runner.tar.gz'
            download(f'https://github.com/actions/runner/releases/download/v{RUNNER_VERSION}/actions-runner-linux-x64-{RUNNER_VERSION}.tar.gz',RUNNER_SHA,archive)
            RUNNER.mkdir(mode=0o700)
            with tarfile.open(archive) as data:
                for member in data.getmembers():safe_member(RUNNER,member.name)
                data.extractall(RUNNER,filter='data')
            shutil.chown(RUNNER,user=USER,group=USER)
            for directory,dirs,files in os.walk(RUNNER):
                for name in dirs+files:shutil.chown(Path(directory)/name,user=USER,group=USER)
    for directory in [HOME/'.cargo',HOME/'ci-failures']:
        directory.mkdir(mode=0o700,exist_ok=True);shutil.chown(directory,user=USER,group=USER)
    run([str(BASE/'cargo/bin/rustup'),'component','add','--toolchain','1.98.1-x86_64-unknown-linux-gnu','rustfmt','clippy'],
        env={**os.environ,'RUSTUP_HOME':str(BASE/'rustup'),'CARGO_HOME':str(BASE/'cargo')})
    print(json.dumps(verify()))


def user_environment():
    return {'PATH':TOOL_PATH,'HOME':str(HOME),'RUSTUP_HOME':str(BASE/'rustup'),'CARGO_HOME':str(HOME/'.cargo'),'CARGO_BUILD_JOBS':'3','CARGO_TARGET_DIR':str(HOME/'native-validator-target'),'XDG_RUNTIME_DIR':f'/run/user/{UID}','LANG':'C.UTF-8'}


def verify():
    tools={}
    environment=user_environment()
    for name,args in {'rustc':['rustc','--version'],'cargo':['cargo','--version'],'rustfmt':['rustfmt','--version'],'clippy':['cargo','clippy','--version'],'bun':['bun','--version'],'protoc':['protoc','--version'],'gh':['gh','--version'],'buildctl':['buildctl','--version']}.items():
        result=run(['/usr/sbin/runuser','-u',USER,'--',*args],env=environment,capture_output=True,text=True)
        tools[name]=result.stdout.splitlines()[0]
    require(tools['rustc'].startswith('rustc 1.98.1 '),'Pinned Rust version mismatch')
    require(tools['bun']=='1.4.3','Pinned Bun version mismatch')
    require(hashlib.sha256((BASE/'buildkit-v0.34.1/bin/buildctl').read_bytes()).hexdigest()==BUILDCTL_SHA,'Qualified BuildKit checksum mismatch')
    return {'runnerName':NAME,'group':GROUP,'uid':UID,'labels':LABELS,'persistent':True,'maximumConcurrentJobs':1,'tools':tools}


def register(payload,write_service=True):
    require(payload.get('group')==GROUP and isinstance(payload.get('token'),str) and len(payload['token'])>=10,'Expected selected-only group and short-lived registration token required')
    require(not (RUNNER/'.runner').exists(),'Runner is already registered; verify existing identity instead of replacing it')
    environment={**user_environment(),'ACTIONS_RUNNER_INPUT_TOKEN':payload['token']}
    run(['/usr/sbin/runuser','-u',USER,'--',str(RUNNER/'config.sh'),'--unattended','--url','https://github.com/spitzli','--runnergroup',GROUP,
         '--name',NAME,'--labels',','.join(LABELS),'--work','_work','--disableupdate'],cwd=RUNNER,env=environment,capture_output=True,text=True)
    if write_service:
        for file in [RUNNER/'.credentials',RUNNER/'.credentials_rsaparams',RUNNER/'.env',RUNNER/'.path']:
            if file.exists():file.chmod(0o600)
        install_service()
    print(json.dumps({'runnerName':NAME,'group':GROUP,'persistent':True,'registered':True}))


def install_service():
    require((RUNNER/'bin/runsvc.sh').is_file(),'Official runner service launcher missing')
    configure_boot()
    SERVICE.write_text(service_unit());SERVICE.chmod(0o644)
    run(['systemctl','daemon-reload'])
    run(['systemd-analyze','verify','/etc/systemd/system/spitzli-plugin-ci-runtime.service','/etc/systemd/system/spitzli-plugin-ci-egress.service','/etc/systemd/system/spitzli-plugin-buildkit.service',str(SERVICE)])
    run(['systemctl','enable','--now','spitzli-plugin-ci-runtime.service','spitzli-plugin-ci-egress.service','spitzli-plugin-buildkit.service'])
    run(['systemctl','enable','--now','spitzli-plugin-runner.service'])


def configure_boot():
    Path('/etc/tmpfiles.d/spitzli-plugin-ci.conf').write_text(f'd /run/user/{UID} 0700 {USER} {USER} -\n')
    Path('/etc/systemd/system/spitzli-plugin-ci-runtime.service').write_text(f'''[Unit]
Description=Prepare only the trusted CI UID runtime directory
After=systemd-tmpfiles-setup.service
Before=spitzli-plugin-buildkit.service spitzli-plugin-runner.service
[Service]
Type=oneshot
ExecStart=/usr/bin/systemd-tmpfiles --create /etc/tmpfiles.d/spitzli-plugin-ci.conf
RemainAfterExit=yes
[Install]
WantedBy=multi-user.target
''')
    directory=Path('/etc/systemd/system/spitzli-plugin-buildkit.service.d')
    directory.mkdir(exist_ok=True)
    (directory/'ci-dependencies.conf').write_text('''[Unit]
Requires=spitzli-plugin-ci-runtime.service spitzli-plugin-ci-egress.service
After=spitzli-plugin-ci-runtime.service spitzli-plugin-ci-egress.service
''')
    target=HOME/'native-validator-target'
    target.mkdir(mode=0o700,exist_ok=True);shutil.chown(target,user=USER,group=USER)
    exports='\n'.join('  export '+key+'='+value for key,value in user_environment().items())
    Path('/etc/profile.d/spitzli-plugin-ci.sh').write_text('if [ "$(id -u)" = "1001" ]; then\n'+exports+'\nfi\n')


def main():
    require(os.geteuid()==0,'Expected root provisioning connection')
    command=sys.argv[1] if len(sys.argv)>1 else ''
    if command=='install':install()
    elif command=='verify':print(json.dumps(verify()))
    elif command=='register':register(json.loads(sys.stdin.read(16384)))
    elif command=='service':install_service()
    else:raise ValueError('Use install, verify, register, or service')

if __name__=='__main__':
    try:main()
    except Exception as error:
        # Config stdout/stderr may contain protocol credentials; never print it or the request.
        print('Qualified CI host setup failed ('+type(error).__name__+'); inspect nonsecret tool/service state.',file=sys.stderr)
        raise SystemExit(1) from None
