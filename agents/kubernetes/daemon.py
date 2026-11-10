#!/usr/bin/env python3
"""Always-on node-local Webdock agent; credentials and Kubernetes access stay on the node."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
from urllib.error import HTTPError
from urllib.parse import urlparse
from connect import agent_request
from observer import collect_observation


def load_state(path,allow_loopback=False):
    path=Path(path)
    stat=path.stat()
    if stat.st_mode & 0o077 or stat.st_uid!=os.getuid():
        raise ValueError('Credentials must be private and owned by the agent user')
    state=json.loads(path.read_text())
    endpoint=urlparse(state['endpoint'])
    local=allow_loopback and endpoint.scheme=='http' and endpoint.hostname in ('localhost','127.0.0.1')
    if (endpoint.scheme!='https' and not local) or not endpoint.hostname or endpoint.username or endpoint.password or endpoint.path not in ('','/') or endpoint.query or endpoint.fragment:
        raise ValueError('Expected a plain HTTPS control origin')
    if not re.fullmatch(r'[1-9][0-9]{0,18}',str(state.get('clusterID',''))) or not re.fullmatch(r'[A-Za-z0-9_-]{43}',state.get('credential','')):
        raise ValueError('Invalid agent identity')
    if type(state.get('generation')) is not int or state['generation']<1 or type(state.get('sequence',0)) is not int or state.get('sequence',0)<0:
        raise ValueError('Invalid agent generation or sequence')
    return state


def save_state(path,state):
    fd,temporary=tempfile.mkstemp(prefix='.connection-',dir=path.parent)
    try:
        with os.fdopen(fd,'w') as stream:
            json.dump(state,stream);stream.flush();os.fsync(stream.fileno())
        os.replace(temporary,path)
        directory=os.open(path.parent,os.O_DIRECTORY)
        try:os.fsync(directory)
        finally:os.close(directory)
    finally:
        if os.path.exists(temporary):os.unlink(temporary)


def heartbeat(path,state):
    observed=collect_observation()
    observed.setdefault('capabilities',{})['provisioningGuardVersion']=1
    state['sequence']=state.get('sequence',0)+1
    observed.update(sequence=state['sequence'],generation=state['generation'])
    save_state(path,state)
    agent_request(state['endpoint'],state,'heartbeat',observed)
    return observed


def execute_local(packet):
    result=subprocess.run([sys.executable,str(Path(__file__).with_name('executor.py'))],input=json.dumps(packet),capture_output=True,text=True,timeout=260,check=True)
    if len(result.stdout)>65536:raise ValueError('Executor result is too large')
    return json.loads(result.stdout)


def run_once(path):
    path=Path(path)
    state=load_state(path)
    observation=heartbeat(path,state)
    database = run_database_once(state, observation, lambda: heartbeat(path,state))
    try:template = run_template_once(state)
    except Exception:
        # Template authorization/transport failures must not starve unrelated lanes.
        # No failed template is executed or acknowledged as successful here.
        print(json.dumps({'event':'template-lane','status':'failed'}),file=sys.stderr)
        template = None
    operation=agent_request(state['endpoint'],state,'claim',{})
    if not operation:
        mail = run_mail_once(path, state, observation)
        if mail is not None: return mail
        configuration = run_mail_configuration(path, state, observation)
        if configuration is not None: return configuration
        return template or database or {'event':'heartbeat','clusterID':state['clusterID'],'nodes':len(observation['nodes'])}
    packet=operation.get('desired')
    valid=packet and packet.get('clusterID')==state['clusterID'] and packet.get('operationID')==operation.get('id') and packet.get('generation')==operation.get('generation')
    outcome={'outcome':'failed','error':'execution_failed'}
    if valid:
        if packet.get('pluginProvisioningGuard') is True:
            packet={**packet,'_guard':{key:state[key] for key in ['endpoint','clusterID','credential']}}
        try:outcome=execute_local(packet)
        except Exception:pass
    # Refresh capacity observation before releasing a completed allocation.
    heartbeat(path,state)
    agent_request(state['endpoint'],state,'complete',{'id':operation['id'],'generation':operation['generation'],**outcome})
    return {'event':'operation','id':operation['id'],'outcome':outcome['outcome']}


def run_template_once(state):
    # Local operator approval is opt-in; existing agents keep their exact polling contract.
    if not Path(os.environ.get('SPITZLI_APP_TEMPLATES','/etc/webdock/app-templates.json')).is_file():return None
    from app_templates import execute
    packet=agent_request(state['endpoint'],state,'template-claim',{})
    if packet is None:return None
    if packet['clusterID']!=state['clusterID']:raise ValueError('Template belongs to another agent')
    identity={key:packet[key] for key in ['operationID','generation','leaseToken','phase']}
    def checkpoint(**value):return agent_request(state['endpoint'],state,'template-checkpoint',{**identity,**value})
    try:
        with open(os.environ.get('SPITZLI_EXECUTOR_LOCK','/run/lock/webdock-managed-executor.lock'),'w') as lock:
            fcntl.flock(lock,fcntl.LOCK_EX)
            proof=execute(packet,checkpoint)
        result={'outcome':'pending'} if proof is None else {'outcome':'succeeded','proof':proof}
    except Exception:
        result={'outcome':'needs-reconciliation'}
    agent_request(state['endpoint'],state,'template-complete',{**identity,**result})
    return {'event':'template-operation','id':packet['operationID'],'outcome':result['outcome']}


def run_database_once(state, observation, refresh):
    # Refresh real observations between slow SQL calls without a second polling thread.
    last_refresh=time.monotonic();refresh_failed=False
    def refresh_if_due():
        nonlocal last_refresh,refresh_failed
        if time.monotonic()-last_refresh>=20:
            try:refresh()
            except Exception:
                refresh_failed=True
                raise
            last_refresh=time.monotonic()
    if observation.get('capabilities',{}).get('databaseVersion')!=1:return None
    from databases import execute_database,validate_packet
    packet=agent_request(state['endpoint'],state,'database-claim',{})
    if packet is None:
        from databases import observe_databases
        configured=agent_request(state['endpoint'],state,'database-config',{})
        if configured.get('installations'):agent_request(state['endpoint'],state,'database-observation',observe_databases(configured))
        return None
    validate_packet(packet)
    if packet['clusterID']!=state['clusterID'] or packet['generation']!=state['generation']:raise ValueError('Database operation belongs to another agent')
    identity={k:packet[k] for k in ['operationID','generation','leaseToken']}
    phase=packet['phase']
    def checkpoint(**value):
        nonlocal phase
        refresh_if_due()
        result=agent_request(state['endpoint'],state,'database-checkpoint' if value else 'database-guard',{**identity,**value})
        phase=value.get('phase',phase)
        return result
    try:
        proof=execute_database(packet,checkpoint)
        if proof is None:
            refresh_if_due()
            return {'event':'database-pending','id':packet['operationID']}
        result={'outcome':'succeeded','proof':proof}
    except Exception as error:
        print(json.dumps({'event':'database-execution-failed','operationID':packet['operationID'],'phase':phase,'exception':type(error).__name__}),file=sys.stderr)
        if refresh_failed:raise
        result={'outcome':'needs-reconciliation'}
    refresh_if_due()
    completed=agent_request(state['endpoint'],state,'database-complete',{**identity,**result})
    if completed.get('pending'):return {'event':'database-pending','id':packet['operationID']}
    return {'event':'database-operation','id':packet['operationID'],'outcome':result['outcome']}


def run_mail_once(path, state, observation):
    capability = observation.get('capabilities', {}).get('nativeMail')
    if not capability:
        return None
    from mail_executor import execute_mail, validate_mail_packet
    packet = agent_request(state['endpoint'], state, 'mail-claim', {})
    if packet is None:
        return None
    validate_mail_packet(packet)
    if packet['clusterID'] != state['clusterID'] or packet['generation'] != state['generation'] or packet['image'] != capability['image']:
        raise ValueError('Mail operation does not belong to this agent')
    identity = {key: packet[key] for key in ('operationID', 'leaseToken', 'generation')}
    def checkpoint(credentials=None):
        body = dict(identity)
        if credentials is not None:
            body['credentials'] = credentials
        return agent_request(state['endpoint'], state, 'mail-checkpoint', body)
    try:
        proof = execute_mail(packet, checkpoint)
        outcome = {'outcome': 'succeeded', 'proof': proof}
    except Exception:
        # An uncertain external mutation requires reconciliation, never an automatic replay.
        outcome = {'outcome': 'needs_review'}
    heartbeat(path, state)
    agent_request(state['endpoint'], state, 'mail-complete', {**identity, **outcome})
    return {'event': 'mail-operation', 'id': packet['operationID'], 'outcome': outcome['outcome']}


def run_mail_configuration(path, state, observation):
    capability = observation.get('capabilities', {}).get('nativeMail', {})
    if capability.get('configurationVersion') != 3:
        return None
    from mail_configuration import execute_configuration, validate_configuration
    packet = agent_request(state['endpoint'],state,'mail-configuration-claim',{})
    if packet is None: return None
    validate_configuration(packet)
    if packet['clusterID']!=state['clusterID'] or packet['generation']!=state['generation']:
        raise ValueError('Mail configuration belongs to another agent')
    identity={key:packet[key] for key in ['operationID','generation','leaseToken']}
    def checkpoint():return agent_request(state['endpoint'],state,'mail-configuration-checkpoint',identity)
    try:outcome={'outcome':'succeeded','proof':execute_configuration(packet,checkpoint)}
    except Exception:outcome={'outcome':'needs_review'}
    heartbeat(path,state)
    agent_request(state['endpoint'],state,'mail-configuration-complete',{**identity,**outcome})
    return {'event':'mail-configuration','id':packet['operationID'],'outcome':outcome['outcome']}


def run_byok_once(path,allow_loopback=False):
    from byok import kubectl, scan, execute
    from observer import observation
    path=Path(path);state=load_state(path,allow_loopback)
    config=agent_request(state['endpoint'],state,'config',{})
    observed=observation(json.loads(kubectl('get','nodes','-o','json')),strict_ready=False)
    state['sequence']=state.get('sequence',0)+1
    observed.update(sequence=state['sequence'],generation=state['generation'])
    save_state(path,state);agent_request(state['endpoint'],state,'heartbeat',observed)
    if config.get('scanRequested') or time.time()-state.get('lastScan',0)>60:
        inventory=scan(config['namespaces'])
        if config.get('clusterUID') and inventory['clusterUID']!=config['clusterUID']:raise ValueError('cluster_identity_changed')
        agent_request(state['endpoint'],state,'inventory',inventory)
        state['lastScan']=time.time();save_state(path,state)
    operation=agent_request(state['endpoint'],state,'byok-claim',{})
    if not operation:return {'event':'heartbeat','clusterID':state['clusterID'],'nodes':len(observed['nodes'])}
    packet=operation['desired']
    outcome={'outcome':'failed'}
    if packet.get('clusterID')==state['clusterID'] and packet.get('operationID')==operation['id'] and packet.get('generation')==operation['generation']:
        try:outcome={'outcome':'succeeded','proof':execute(packet,config['namespaces'])}
        except Exception:pass
    agent_request(state['endpoint'],state,'byok-complete',{'id':operation['id'],'generation':operation['generation'],**outcome})
    state['lastScan']=0;save_state(path,state)
    return {'event':'operation','id':operation['id'],'outcome':outcome['outcome']}


def authentication_revoked(error):
    # A provider firewall/proxy 401/403 is not a revoked Webdock credential.
    # Disabled BYOK policy also resumes automatically once it is enabled again.
    if error.code!=401:return False
    try:
        body=error.read(8193)
        if len(body)>8192:return False
        data=json.loads(body)
        return isinstance(data,dict) and data.get('error')=='Cluster authentication failed.'
    except Exception:return False


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--credentials',required=True)
    parser.add_argument('--interval',type=int,default=15)
    parser.add_argument('--once',action='store_true')
    parser.add_argument('--byok',action='store_true')
    parser.add_argument('--allow-loopback',action='store_true',help='Explicit local development only')
    args=parser.parse_args()
    if not 10<=args.interval<=300:parser.error('Interval must be between 10 and 300 seconds')
    path=Path(args.credentials)
    try:load_state(path,args.allow_loopback)
    except Exception:parser.exit(78,'Invalid private agent configuration.\n')
    with path.with_suffix('.lock').open('a') as lock:
        fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
        while True:
            try:print(json.dumps(run_byok_once(path,args.allow_loopback) if args.byok else run_once(path)),flush=True)
            except HTTPError as error:
                status=error.code;revoked=authentication_revoked(error);error.close()
                if revoked:parser.exit(78,'Agent authorization rejected. Re-enrollment is required.\n')
                print(json.dumps({'event':'control-unavailable','status':status}),flush=True)
            except Exception:
                print(json.dumps({'event':'control-unavailable'}),flush=True)
            if args.once:break
            time.sleep(args.interval)


if __name__=='__main__':main()
