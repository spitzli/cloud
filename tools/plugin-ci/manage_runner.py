#!/usr/bin/env python3
"""Local operator orchestration; GitHub credentials never leave this workstation."""
from __future__ import annotations
import argparse
import base64
import json
from pathlib import Path
import subprocess

HOST='root@167.86.75.144'
GROUP='cloud-plugin-trusted-eu2'
NAME='cloud-plugin-eu2-trusted-01'
REPOSITORIES=['cloud-plugin-'+name for name in ['archives','echo-demo','github-repository','hosting','lumaserv-compute','mail','mail-relay','object-storage','postgres','sdk']]
SSH=['ssh','-i',str(Path.home()/'.ssh/id_ed25519'),'-o','IdentityAgent=none','-o','IdentitiesOnly=yes','-o','BatchMode=yes','-o','StrictHostKeyChecking=yes',HOST]
CMS='cloud-plugin-managed-cms'
GATE="github.event_name != 'pull_request' || github.event.pull_request.head.repo.full_name == github.repository"
RUNS_ON='${{ fromJSON(vars.WEBDOCK_PLUGIN_CI_RUNNER) }}'


def api(path,method='GET',body=None):
    args=['gh','api',path,'--method',method]
    if body is not None:args+=['--input','-']
    result=subprocess.run(args,input=None if body is None else json.dumps(body),capture_output=True,text=True)
    if result.returncode:raise ValueError('GitHub API request failed: '+path)
    return json.loads(result.stdout) if result.stdout.strip() else None


def group():
    groups=api('orgs/spitzli/actions/runner-groups')['runner_groups']
    result=next((item for item in groups if item['name']==GROUP),None)
    if result is None:
        result=api('orgs/spitzli/actions/runner-groups','POST',{'name':GROUP,'visibility':'selected','selected_repository_ids':[],'allows_public_repositories':True})
    if result['visibility']!='selected' or not result['allows_public_repositories']:
        raise ValueError('Expected selected-only public-repository runner group')
    return result


def workflow_gate(raw):
    import yaml
    workflow=yaml.load(raw,Loader=yaml.BaseLoader)
    events=workflow.get('on',{})
    event_names=set(events if isinstance(events,(dict,list)) else [events])
    if not event_names or not event_names <= {'push','pull_request','workflow_dispatch'}:
        raise ValueError('Unreviewed workflow trigger on trusted runner')
    jobs=workflow.get('jobs',{})
    if not jobs:raise ValueError('Workflow has no reviewed jobs')
    for job in jobs.values():
        condition=job.get('if','')
        if condition not in {GATE, "github.event_name == 'push'", "startsWith(github.ref, 'refs/tags/v') && github.event_name == 'push'"}:
            raise ValueError('Every trusted-runner job must exclude fork pull requests')
        if job.get('runs-on','')!=RUNS_ON:
            raise ValueError('Every trusted-runner job must use the reviewed runner labels')
    return True


def verify_repository(name,already_selected=False):
    if name not in REPOSITORIES+[CMS]:raise ValueError('Repository outside reviewed platform allowlist')
    repository=api('repos/spitzli/'+name)
    permissions=api('repos/spitzli/'+name+'/actions/permissions')
    if permissions['enabled'] and not already_selected:raise ValueError('Actions must remain disabled until selected-group and variable verification: '+name)
    entries=api('repos/spitzli/'+name+'/contents/.github/workflows')
    files=[entry for entry in entries if entry['name'].endswith(('.yml','.yaml'))]
    if not files:raise ValueError('No reviewed workflow found: '+name)
    for entry in files:
        content=api('repos/spitzli/'+name+'/contents/'+entry['path'])
        workflow_gate(base64.b64decode(content['content']).decode())
    return {'id':repository['id'],'name':repository['full_name'],'defaultBranch':repository['default_branch'],'actionsEnabled':permissions['enabled'],'verifiedWorkflows':[entry['name'] for entry in files]}


def register():
    current=group();path='orgs/spitzli/actions/runner-groups/'+str(current['id'])
    runners=api(path+'/runners')['runners']
    if runners:
        if len(runners)!=1 or runners[0]['name']!=NAME:raise ValueError('Unexpected runner already in selected group')
        return {'groupID':current['id'],'runner':runners[0],'registeredNow':False}
    if api(path+'/repositories')['total_count']!=0:raise ValueError('Initial registration requires empty selected group')
    # Only the one-hour registration token crosses SSH stdin; no PAT, OAuth token or operator key is transferred.
    token=api('orgs/spitzli/actions/runners/registration-token','POST')['token']
    result=subprocess.run(SSH+['python3','/opt/spitzli-plugin-ci/qualified_host.py','register'],input=json.dumps({'group':GROUP,'token':token}),capture_output=True,text=True)
    del token
    if result.returncode:raise ValueError('Remote registration failed; inspect private runner service state')
    return {'groupID':current['id'],'runnerName':NAME,'registeredNow':True}


def select(include_cms=False):
    current=group()
    selected_names={item['name'] for item in api('orgs/spitzli/actions/runner-groups/'+str(current['id'])+'/repositories')['repositories']}
    if selected_names-set(REPOSITORIES+[CMS]):
        raise ValueError('Selected group contains additional owner-managed repositories; refusing to replace its allowlist')
    include_cms=include_cms or CMS in selected_names
    reviewed=[verify_repository(name,name in selected_names) for name in REPOSITORIES+([CMS] if include_cms else [])]
    path='orgs/spitzli/actions/runner-groups/'+str(current['id'])+'/repositories'
    api(path,'PUT',{'selected_repository_ids':[item['id'] for item in reviewed]})
    selected=api(path)['repositories']
    if {item['full_name'] for item in selected}!={item['name'] for item in reviewed}:raise ValueError('Selected group readback mismatch')
    return {'groupID':current['id'],'selectedRepositories':reviewed,'repositoryActionsEnabled':any(item['actionsEnabled'] for item in reviewed),'newRepositoryActionsDisabled':True}


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command',choices=['register','verify','select'])
    parser.add_argument('--include-cms',action='store_true',help='Only after CMS repository/workflows are reviewed and Actions are still disabled')
    options=parser.parse_args()
    command=options.command
    if command=='register':result=register()
    elif command=='select':result=select(options.include_cms)
    else:
        current=group();path='orgs/spitzli/actions/runner-groups/'+str(current['id'])
        result={'groupID':current['id'],'group':GROUP,'visibility':current['visibility'],'runners':api(path+'/runners')['runners'],'selectedRepositories':[item['full_name'] for item in api(path+'/repositories')['repositories']]}
    print(json.dumps(result,indent=2))

if __name__=='__main__':
    try:main()
    except Exception as error:raise SystemExit(type(error).__name__+': '+str(error)) from None
