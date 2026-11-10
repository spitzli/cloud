"""Typed CE domain/group/user provisioning. Never adopts unowned accounts or deletes mail."""
import re
from datetime import datetime, timezone

# Interactive identities edit mail; only the separately authorized SMTP submission service sends.
MAIL_PERMISSIONS = ['authenticate', 'jmapCoreEcho', 'jmapMailboxGet', 'jmapMailboxChanges',
    'jmapMailboxQuery', 'jmapMailboxQueryChanges', 'jmapMailboxCreate', 'jmapMailboxUpdate', 'jmapMailboxDestroy',
    'jmapThreadGet', 'jmapThreadChanges', 'jmapEmailGet', 'jmapEmailChanges', 'jmapEmailQuery', 'jmapEmailQueryChanges',
    'jmapEmailCreate', 'jmapEmailUpdate', 'jmapEmailDestroy', 'jmapEmailImport', 'jmapEmailParse', 'jmapSearchSnippetGet',
    'jmapIdentityGet', 'jmapIdentityChanges', 'jmapEmailSubmissionGet', 'jmapEmailSubmissionChanges',
    'jmapEmailSubmissionQuery', 'jmapEmailSubmissionQueryChanges',
    'jmapQuotaGet', 'jmapQuotaQuery', 'jmapBlobGet', 'jmapBlobUpload', 'jmapBlobLookup']
NATIVE_DKIM = {'@type': 'Automatic', 'algorithms': {'Dkim1RsaSha256': True},
               'selectorTemplate': 'webdock-{algorithm}-{date-%Y%m%d}'}


def validate_configuration(p):
    def identifier(value):
        if not isinstance(value, str) or not re.fullmatch(r'[1-9][0-9]{0,18}', value) or int(value) > 9223372036854775807:
            raise ValueError('Invalid Mail configuration identifier')
    def domain(value):
        if not isinstance(value, str) or len(value) > 253 or not re.fullmatch(r'(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z0-9-]{2,63}', value):
            raise ValueError('Invalid Mail domain')
    try:
        if set(p) != {'version','customerID','clusterID','operationID','generation','revision','leaseToken','leaseUntil','hostname','credentials','domains','mailboxes','identities','submission'} or type(p['version']) is not int or p['version'] != 3:
            raise ValueError('Invalid Mail configuration packet')
        if p['submission'] is not None:
            from mail_submission import validate_submission
            validate_submission(p['submission'])
            if p['submission']['customerID']!=p['customerID'] or p['submission']['hostname']!=p['hostname']:raise ValueError('Submission instance changed')
        for key in ['customerID','clusterID','operationID','revision']: identifier(p[key])
        if type(p['generation']) is not int or p['generation'] < 1: raise ValueError('Invalid Mail generation')
        if not re.fullmatch(r'[A-Za-z0-9_-]{32,64}',p['leaseToken']): raise ValueError('Invalid Mail lease')
        if datetime.fromisoformat(p['leaseUntil'].replace('Z','+00:00')) <= datetime.now(timezone.utc): raise ValueError('Expired Mail lease')
        domain(p['hostname'])
        if not p['hostname'].startswith('mail-'+p['customerID']+'.'): raise ValueError('Invalid Mail instance hostname')
        if set(p['credentials']) != {'username','password'}: raise ValueError('Invalid Mail administrator')
        for value in p['credentials'].values():
            if not isinstance(value,str) or not 1 <= len(value) <= 1024 or any(ord(c)<32 for c in value): raise ValueError('Invalid Mail credential')
        for field,limit,keys in [('domains',32,{'id','name','providerId','ownershipVerified'}),
                                 ('mailboxes',64,{'id','domainId','localPart','quotaBytes','providerId'}),
                                 ('identities',128,{'id','subject','providerId','enabled','mailboxIds','username','password'})]:
            if not isinstance(p[field],list) or len(p[field])>limit: raise ValueError('Mail configuration is too large')
            seen=set()
            for item in p[field]:
                if set(item)!=keys: raise ValueError('Unexpected Mail configuration field')
                identifier(item['id'])
                if item['id'] in seen: raise ValueError('Duplicate Mail resource')
                seen.add(item['id'])
                if item['providerId'] is not None and not re.fullmatch(r'[A-Za-z0-9_-]{1,64}',item['providerId']): raise ValueError('Invalid Mail provider reference')
        domains={item['id'] for item in p['domains']}; mailboxes={item['id'] for item in p['mailboxes']}
        for item in p['domains']:
            domain(item['name'])
            if type(item['ownershipVerified']) is not bool: raise ValueError('Invalid Mail domain ownership')
        for item in p['mailboxes']:
            if item['domainId'] not in domains or not re.fullmatch(r'[a-z0-9]+(?:[._+-][a-z0-9]+)*',item['localPart']) or len(item['localPart'])>64: raise ValueError('Invalid Mail address')
            if type(item['quotaBytes']) is not int or not 1048576<=item['quotaBytes']<=8589934592: raise ValueError('Invalid Mail quota')
        if sum(item['quotaBytes'] for item in p['mailboxes'])>8589934592: raise ValueError('Mail instance capacity exceeded')
        for item in p['identities']:
            identifier(item['subject'])
            if type(item['enabled']) is not bool or not isinstance(item['mailboxIds'],list) or len(item['mailboxIds'])>64 or len(set(item['mailboxIds']))!=len(item['mailboxIds']) or not set(item['mailboxIds']).issubset(mailboxes): raise ValueError('Invalid Mail grants')
            if item['enabled'] != bool(item['mailboxIds']) or item['username'] != 'wd-'+item['id']+'@'+p['hostname']: raise ValueError('Invalid Mail identity')
            if not isinstance(item['password'],str) or not 16<=len(item['password'])<=128 or not re.fullmatch(r'[A-Za-z0-9_!@.-]+',item['password']): raise ValueError('Invalid Mail identity credential')
    except (KeyError, TypeError, AttributeError, OverflowError):
        raise ValueError('Invalid Mail configuration packet') from None


def apply_configuration(client, packet, guard):
    validate_configuration(packet)
    def call(method,args):
        guard()
        return client.call(method,args)
    def exact(rows, provider_id, name, domain_id=None):
        candidates=[row for row in rows if row.get('id')==provider_id] if provider_id else [row for row in rows if row.get('name')==name and (domain_id is None or row.get('domainId')==domain_id)]
        if len(candidates)>1: raise ValueError('Ambiguous Mail ownership')
        if provider_id and not candidates: raise ValueError('Missing existing Mail resource; restore before continuing')
        row=candidates[0] if candidates else None
        if row and (row.get('name')!=name or domain_id is not None and row.get('domainId')!=domain_id): raise ValueError('Mail ownership changed')
        return row
    def created(result):
        value=result.get('created',{}).get('resource',{}).get('id')
        if not isinstance(value,str) or not re.fullmatch(r'[A-Za-z0-9_-]{1,64}',value): raise ValueError('Mail create outcome unknown')
        return value
    domains=call('x:Domain/get',{'ids':None})['list']
    internal=exact(domains,None,packet['hostname'])
    if not internal: raise ValueError('Mail identity domain is missing')
    proof={'domains':{},'mailboxes':{},'identities':{},'dkim':{},'submission':None}
    for item in packet['domains']:
        row=exact(domains,item['providerId'],item['name'])
        if not row:
            if not item['ownershipVerified']: raise ValueError('Verify Mail domain ownership first')
            value=created(call('x:Domain/set',{'create':{'resource':{'name':item['name'],'dkimManagement':NATIVE_DKIM}}}))
            row=exact(call('x:Domain/get',{'ids':[value]})['list'],value,item['name'])
        proof['domains'][item['id']]=row['id']
    def account(kind,item,name,domain_id,desired):
        if item['providerId']:
            rows=call('x:Account/get',{'ids':[item['providerId']]})['list']
        else:
            query=call('x:Account/query',{'filter':{'domainId':domain_id},'limit':256,'calculateTotal':True})
            if query.get('total',len(query['ids']))>256: raise ValueError('Mail account inventory is too large')
            rows=call('x:Account/get',{'ids':query['ids']})['list'] if query['ids'] else []
        row=exact(rows,item['providerId'],name,domain_id)
        marker='webdock:'+kind+':'+packet['customerID']+':'+item['id']
        account_type='Group' if kind=='mailbox' else 'User'
        if row and (row.get('@type')!=account_type or row.get('description')!=marker): raise ValueError('Mail account ownership cannot be verified')
        if row:
            # The original login credential is retained; retries never rotate it.
            change={k:v for k,v in desired.items() if k!='credentials'}
            call('x:Account/set',{'update':{row['id']:change}})
            value=row['id']
        else:
            value=created(call('x:Account/set',{'create':{'resource':{'@type':account_type,'name':name,'domainId':domain_id,'description':marker,**desired}}}))
        observed=exact(call('x:Account/get',{'ids':[value]})['list'],value,name,domain_id)
        if observed.get('description')!=marker or observed.get('@type')!=account_type: raise ValueError('Mail account ownership changed')
        for key in ['quotas','permissions','memberGroupIds']:
            if key in desired and observed.get(key,{})!=desired[key]: raise ValueError('Mail account configuration was not confirmed')
        return value
    for item in packet['mailboxes']:
        proof['mailboxes'][item['id']]=account('mailbox',item,item['localPart'],proof['domains'][item['domainId']],
            {'roles':{'@type':'Default'},'permissions':{'@type':'Inherit'},'quotas':{'maxDiskQuota':item['quotaBytes']}})
    for item in packet['identities']:
        enabled={permission:True for permission in MAIL_PERMISSIONS} if item['enabled'] else {}
        proof['identities'][item['id']]=account('identity',item,'wd-'+item['id'],internal['id'],
            {'roles':{'@type':'User'},'permissions':{'@type':'Replace','enabledPermissions':enabled,'disabledPermissions':{}},
             'quotas':{'maxDiskQuota':1048576},'encryptionAtRest':{'@type':'Disabled'},
             'credentials':{'0':{'@type':'Password','secret':item['password']}},
             'memberGroupIds':{proof['mailboxes'][value]:True for value in item['mailboxIds']}})
    import time
    import base64
    for item in packet['domains']:
        domain_id = proof['domains'][item['id']]
        row = exact(call('x:Domain/get', {'ids': [domain_id]})['list'], domain_id, item['name'])
        current = row.get('dkimManagement', {})
        desired = NATIVE_DKIM
        if any(current.get(key) != value for key, value in desired.items()):
            call('x:Domain/set', {'update': {domain_id: {'dkimManagement': desired}}})
        until = time.monotonic() + 20
        while True:
            rows = call('x:DkimSignature/get', {'ids': None, 'properties': ['id', 'domainId', 'selector', 'publicKey', 'stage']})['list']
            keys = []
            for key in rows:
                if key.get('domainId') != domain_id or key.get('stage') != 'active' or not re.fullmatch(r'(?:webdock-rsa-|v1-rsa-)[a-z0-9-]{1,40}', key.get('selector', '')):
                    continue
                public = ''.join(key['publicKey'].split())
                if not 128 <= len(base64.b64decode(public, validate=True)) <= 1024:
                    raise ValueError('Invalid native DKIM public key')
                keys.append({'id': key['id'], 'selector': key['selector'], 'publicKey': public})
            if keys:
                if len(keys) > 2: raise ValueError('Review active DKIM keys before continuing')
                proof['dkim'][item['id']] = sorted(keys, key=lambda key: key['id'])
                break
            if time.monotonic() >= until: raise ValueError('Native DKIM generation was not confirmed')
            time.sleep(.2)
    return proof


def execute_configuration(packet, checkpoint):
    import fcntl
    import os
    import time
    from executor import get, check_owner, lease
    from mail_client import MailClient
    validate_configuration(packet)
    last=0; started=time.monotonic()
    def guard():
        nonlocal last
        if time.monotonic()-started > 600: raise ValueError('Mail configuration deadline exceeded')
        lease(packet)
        if time.monotonic()-last > 20:
            packet['leaseUntil']=checkpoint()['leaseUntil'];lease(packet);last=time.monotonic()
    with open(os.environ.get('SPITZLI_EXECUTOR_LOCK','/run/lock/webdock-managed-executor.lock'),'a') as lock:
        fcntl.flock(lock,fcntl.LOCK_EX)
        guard()
        namespace='wd-mail-'+packet['customerID'];name='app-'+packet['customerID']
        owner={'projectID':packet['customerID'],'appID':packet['customerID'],'clusterID':packet['clusterID']}
        service=get('service',name,namespace);deployment=get('deployment',name,namespace)
        if not service or not deployment: raise ValueError('Mail instance is unavailable')
        for resource in [service,deployment]:
            check_owner(resource,owner)
            if resource['metadata']['labels'].get('webdock.dev/app-id')!=packet['customerID']:raise ValueError('Mail instance ownership changed')
        if deployment['spec']['replicas']!=1 or deployment.get('status',{}).get('availableReplicas',0)!=1:raise ValueError('Mail instance is not ready')
        env=deployment['spec']['template']['spec']['containers'][0].get('env',[])
        if any(e['name'].startswith('STALWART_RECOVERY') for e in env) or not any(e['name']=='STALWART_PUBLIC_URL' and e.get('value')=='https://'+packet['hostname'] for e in env):raise ValueError('Mail instance configuration changed')
        address=service['spec']['clusterIP']
        client=MailClient('http://'+('['+address+']' if ':' in address else address)+':8080',**packet['credentials'])
        proof=apply_configuration(client,packet,guard)
        if packet['submission'] is not None:
            from mail_submission import execute_submission
            try:proof['submission']=execute_submission(client,packet,deployment,service,guard)
            except Exception:proof['submission']={'ready':False}
        return proof
