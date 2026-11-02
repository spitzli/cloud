#!/usr/bin/env python3
"""Exercise the real Docker frontend's backend-specific ignore rules; execute no source."""
from pathlib import Path
import json
import shutil
import subprocess
import tempfile

ROOT=Path(__file__).resolve().parents[2]

def main():
    with tempfile.TemporaryDirectory(prefix='webdock-backend-context-') as owned:
        root=Path(owned);context=root/'context';context.mkdir(mode=0o700)
        for name in ['backend.Dockerfile','backend.Dockerfile.dockerignore']:
            shutil.copyfile(ROOT/'deploy/docker'/name,context/name)
        public='crates/context-fixture/src/lib.rs'
        (context/public).parent.mkdir(parents=True)
        (context/public).write_text('// public compilation input\n')
        forbidden=['.env.local','crates/context-fixture/.env.production','crates/context-fixture/live.credentials.json','crates/context-fixture/authority.private.json','crates/context-fixture/issuer.key','crates/context-fixture/cluster.kubeconfig','crates/context-fixture/state.sqlite','crates/context-fixture/node_modules/private.js','apps/console/frontend.ts','packages/frontend.ts']
        for name in forbidden:
            path=context/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_text('NEVER_SEND_RUNTIME_CREDENTIAL_SENTINEL\n')
        output=root/'export'
        result=subprocess.run(['docker','buildx','build','--platform','linux/amd64','--file',str(context/'backend.Dockerfile'),'--target','source-context','--output','type=local,dest='+str(output),str(context)],capture_output=True,timeout=120)
        if result.returncode:raise SystemExit('Owned backend context frontend check failed')
        assert (output/'src'/public).read_text()=='// public compilation input\n'
        for name in forbidden:assert not (output/'src'/name).exists(),name
        assert not any(b'NEVER_SEND_RUNTIME_CREDENTIAL_SENTINEL' in p.read_bytes() for p in output.rglob('*') if p.is_file())
        print(json.dumps({'passed':True,'actualDockerFrontend':True,'sourceExecuted':False,'privateInputsExcluded':len(forbidden),'frontendExcluded':True,'temporaryContextRemovedOnExit':True}))

if __name__=='__main__':main()
