"""Operator-installed immutable registry allowlist; never carries credentials in packets."""
import json
import os
from pathlib import Path
import re
import stat

class RegistryError(Exception):pass


def pull_configuration(packet,config=None):
    if config is None:
        location=os.environ.get('SPITZLI_REGISTRY_CONFIG')
        if not location:return None
        path=Path(location);info=path.lstat()
        if not stat.S_ISREG(info.st_mode) or info.st_uid!=0 or info.st_mode & 0o022:raise RegistryError('registry_setup_required')
        config=json.loads(path.read_text())
    if config.get('version')!=1 or not config.get('euStorageEvidence'):raise RegistryError('registry_setup_required')
    projects=config.get('projects',{})
    image=packet['spec']['image']
    managed_hosts={p.get('repository','').split('/')[0].lower() for p in projects.values()}
    # Foreign cached images retain Never; cached private registry images still require project ownership.
    if image.split('/')[0].lower() not in managed_hosts:return None
    project=projects.get(packet['projectID'])
    if not project:raise RegistryError('registry_image_denied')
    repository=project.get('repository','');secret=project.get('pullSecret','')
    if not re.fullmatch(r'[a-z0-9.-]+(?::[0-9]+)?/[a-z0-9/_-]+',repository) or '..' in repository or not re.fullmatch(r'[a-z0-9][a-z0-9-]{0,62}',secret) or project.get('readOnly') is not True or not project.get('credentialScopeEvidence'):
        raise RegistryError('registry_setup_required')
    if not re.fullmatch(re.escape(repository)+r'@sha256:[0-9a-f]{64}',packet['spec']['image']):raise RegistryError('registry_image_denied')
    return {'name':secret}


def capability(kubectl,runner=None):
    """Only advertise installed policy with reachable Kubernetes pull-secret objects."""
    import subprocess
    import time
    runner=runner or subprocess.run
    location=os.environ.get('SPITZLI_REGISTRY_CONFIG')
    if not location:return False
    try:
        path=Path(location);info=path.lstat()
        if not stat.S_ISREG(info.st_mode) or info.st_uid!=0 or info.st_mode & 0o022:return False
        config=json.loads(path.read_text());projects=config.get('projects',{})
        if not projects or len(projects)>32:return False
        deadline=time.monotonic()+10
        for project_id,settings in projects.items():
            if not re.fullmatch(r'[1-9][0-9]{0,18}',project_id):return False
            packet={'projectID':project_id,'spec':{'image':settings['repository']+'@sha256:'+'0'*64}}
            pull=pull_configuration(packet,config)
            for namespace in ('wd-'+project_id,'wdv-'+project_id):
                remaining=deadline-time.monotonic()
                if remaining<=0:return False
                result=runner([*kubectl,'get','secret',pull['name'],'-n',namespace,'-o','jsonpath={.type}'],check=True,capture_output=True,text=True,timeout=min(5,remaining))
                if result.stdout.strip()!='kubernetes.io/dockerconfigjson':return False
        return True
    except Exception:return False
