import copy
import unittest
from unittest.mock import patch
from datetime import datetime, timezone, timedelta
import app_templates as t


def packet():
    return {'version':1,'templateID':'managed-cms','operationID':'10','generation':1,'leaseToken':'a'*43,
      'leaseUntil':(datetime.now(timezone.utc)+timedelta(seconds=60)).isoformat(), 'phase':'migration',
      'projectID':'20','clusterID':'30','appID':'40','instanceID':'50','namespace':'wd-20','revision':1,
      'spec':{'image':'ghcr.io/example/cms@sha256:'+'a'*64,'cpuMillicores':250,'memoryBytes':536870912,'ephemeralBytes':134217728,'volumeBytes':1073741824,'port':3000,'healthPath':'/api/health','replicas':1,'args':[]},
      'quota':{'cpuMillicores':250,'memoryBytes':536870912,'ephemeralBytes':134217728,'volumeBytes':1073741824,'apps':1},
      'environment':{'SPITZLI_AUTH_ISSUER':'https://auth.example.com/api/auth','CMS_INSTANCE_ID':'50','CMS_PROJECT_ID':'20','CMS_ORIGIN':'https://50.cms.example.com','PAYLOAD_SECRET':'a'*43,'CMS_BRIDGE_SECRET':'b'*43,'CMS_MIGRATING':'true','CMS_MIGRATION_DATABASE_URL':'postgresql://wd_migrate_60:secret@postgres-rw.wd-db-70.svc.cluster.local:5432/db?sslmode=verify-full','DATABASE_CA_CERT':'certificate'},
      'backendID':'70','origin':'https://50.cms.example.com','pinnedAddresses':['192.0.2.1'],'jobUID':None,'intentRecorded':False,'storageState':'unallocated','certificateUID':None,'ingressUID':None}


def config():return {'image':packet()['spec']['image'],'domain':'cms.example.com','addresses':['192.0.2.1'],'certificateIssuer':'cms-acme','ingressClass':'traefik','authIssuer':'https://auth.example.com/api/auth','authAddresses':['192.0.2.2']}


class TemplatesTest(unittest.TestCase):
    def test_approval_is_exact_and_packet_cannot_choose_commands(self):
        t.validate(packet(),config())
        for change in [{'origin':'https://evil.example.com'},{'command':['sh']},{'spec':{**packet()['spec'],'image':'nginx:latest'}},{'namespace':'default'},{'pinnedAddresses':['127.0.0.1']}]:
            with self.subTest(change=change),self.assertRaises(ValueError):t.validate({**packet(),**change},config())

    def test_bounded_job_separates_credentials_and_retains_media(self):
        secret,job=t.migration_manifests(packet())
        self.assertNotIn('DATABASE_URL',secret['stringData'])
        self.assertEqual(job['spec']['backoffLimit'],0)
        self.assertEqual(job['spec']['activeDeadlineSeconds'],240)
        pod=job['spec']['template']['spec'];container=pod['containers'][0]
        self.assertEqual(pod['securityContext']['runAsUser'],65532)
        self.assertTrue(container['securityContext']['readOnlyRootFilesystem'])
        self.assertEqual(container['command'],['node','/app/migrate.mjs'])
        for name in ['livenessProbe','readinessProbe','startupProbe']:self.assertIn(name,container)
        self.assertEqual(pod['volumes'][1]['persistentVolumeClaim']['claimName'],'app-40-data')
        self.assertNotIn('ttlSecondsAfterFinished',job['spec'])

    def test_resume_missing_job_never_replays_after_intent(self):
        p={**packet(),'intentRecorded':True}
        with patch.object(t.e,'get',return_value=None),self.assertRaisesRegex(ValueError,'reconciliation'):
            t.migrate(p,lambda **kw:None)

    def test_job_uid_and_generation_ownership_guard(self):
        p=packet();p['jobUID']='original'
        _,job=t.migration_manifests(p);job['metadata']['uid']='replacement'
        with patch.object(t.e,'get',return_value=job),self.assertRaises(ValueError):t.migrate(p,lambda **kw:None)

    def test_route_only_exact_host_and_service(self):
        ingress=t.publication_manifest(packet(),config())
        self.assertEqual(ingress['spec']['rules'][0]['host'],'50.cms.example.com')
        self.assertEqual(ingress['spec']['tls'],[{'hosts':['50.cms.example.com'],'secretName':'app-40-tls'}])
        self.assertEqual(ingress['spec']['rules'][0]['http']['paths'][0]['backend']['service']['name'],'app-40')

    def test_expired_lease_denies_mutations(self):
        p=packet();p['leaseUntil']='2000-01-01T00:00:00Z'
        with self.assertRaises(Exception):t.validate(p,config())

    def test_revoked_guard_prevents_namespace_or_job_creation(self):
        def guard(**value):raise PermissionError('revoked')
        with patch.object(t.e,'get',return_value=None),patch.object(t.e,'prepare_namespace') as namespace,patch.object(t.e,'kubectl') as kubectl,self.assertRaises(PermissionError):t.migrate(packet(),guard)
        namespace.assert_not_called();kubectl.assert_not_called()

    def test_failed_job_retains_database_and_media(self):
        p=packet();_,job=t.migration_manifests(p);job['metadata']['uid']='uid';job['status']={'failed':1}
        with patch.object(t.e,'get',return_value=job),patch.object(t.e,'guarded_delete') as delete,self.assertRaisesRegex(ValueError,'retained'):t.migrate(p,lambda **value:None)
        delete.assert_not_called()

    def test_runtime_dml_url_cannot_enter_initialization(self):
        p=packet();p['environment']['DATABASE_URL']='postgresql://runtime'
        with self.assertRaises(ValueError):t.validate(p,config())

    def test_foreign_origin_is_not_adopted(self):
        p=packet();deployment,_=t.e.manifests(p);deployment['metadata'].update(uid='uid',generation=1);deployment['status']={'availableReplicas':1,'observedGeneration':1}
        route=t.publication_manifest(p,config());route['metadata']['namespace']='foreign'
        with patch.object(t.e,'get',side_effect=lambda kind,*args:deployment if kind=='deployment' else None),patch.object(t.e,'kubectl',return_value=t.json.dumps({'items':[route]})),patch.object(t.e,'apply') as apply,self.assertRaisesRegex(ValueError,'already owned'):t.publish(p,config(),lambda **value:None)
        apply.assert_not_called()

    def test_dns_drift_never_connects_to_unapproved_address(self):
        with patch.object(t.socket,'getaddrinfo',return_value=[(2,1,6,'',('192.0.2.99',443))]),patch.object(t.socket,'create_connection') as connect,self.assertRaises(ValueError):t.tls_health(packet())
        connect.assert_not_called()

    def test_fresh_bound_app_revision_two_allocates_once_with_durable_intent(self):
        p={**packet(),'revision':2,'storageState':'unallocated'};events=[]
        def checkpoint(**value):events.append(value)
        def prepare(value,initial_allocation=False):
            events.append(('storage',initial_allocation,value['revision']))
            if not initial_allocation:raise ValueError('Missing volume; refusing empty replacement')
        with patch.object(t.e,'get',return_value=None),patch.object(t.e,'prepare_storage',side_effect=prepare):
            t.prepare_initial_storage(p,checkpoint)
        self.assertEqual(events,[{'storageState':'allocating'},('storage',True,2),{'storageState':'ready'}])

    def test_retained_or_uncertain_storage_never_allows_empty_replacement(self):
        for status in ['allocating','ready','unknown']:
            p={**packet(),'revision':2,'storageState':status}
            with self.subTest(status=status),patch.object(t.e,'get',return_value=None),patch.object(t.e,'prepare_storage',side_effect=ValueError('Missing volume')) as prepare,self.assertRaises(ValueError):t.prepare_initial_storage(p,lambda **value:None)
            self.assertFalse(prepare.call_args.kwargs.get('initial_allocation',False))

    def test_retained_storage_can_resume_without_reallocation(self):
        p={**packet(),'revision':2,'storageState':'allocating'};events=[]
        with patch.object(t.e,'get',return_value=None),patch.object(t.e,'prepare_storage') as prepare:t.prepare_initial_storage(p,lambda **value:events.append(value))
        self.assertFalse(prepare.call_args.kwargs.get('initial_allocation',False));self.assertEqual(events[-1],{'storageState':'ready'})

    def test_existing_pvc_prevents_fresh_allocation_even_without_state(self):
        p={**packet(),'revision':2,'storageState':'unallocated'}
        with patch.object(t.e,'get',return_value={'metadata':{}}),patch.object(t.e,'prepare_storage',side_effect=ValueError('Missing volume')) as prepare,self.assertRaises(ValueError):t.prepare_initial_storage(p,lambda **value:None)
        self.assertFalse(prepare.call_args.kwargs.get('initial_allocation',False))

    def test_prepare_bind_initialize_revision_two_uses_first_allocation_path(self):
        p={**packet(),'revision':2};created=[];checkpoints=[]
        _,job=t.migration_manifests(p);job['metadata']['uid']='initial-job'
        quota={'metadata':{'labels':t.e.labels(p,False)},'spec':{'hard':{'count/jobs.batch':'0'}}}
        def get(kind,name,namespace=None):
            if kind=='job':return job if created else None
            if kind=='resourcequota':return quota
            return None
        def prepare(value,initial_allocation=False):
            self.assertEqual(value['revision'],2)
            self.assertTrue(initial_allocation)
            self.assertIn({'storageState':'allocating'},checkpoints)
        with patch.object(t.e,'get',side_effect=get),patch.object(t.e,'prepare_namespace'),patch.object(t.e,'prepare_storage',side_effect=prepare),patch.object(t.e,'apply'),patch.object(t.e,'kubectl',side_effect=lambda *args,**kwargs:created.append(True)):
            self.assertIsNone(t.migrate(p,lambda **value:checkpoints.append(value)))
        self.assertIn({'storageState':'ready'},checkpoints)
        self.assertEqual(checkpoints[-1],{'jobUID':'initial-job'})

    def test_certificate_is_exact_owned_and_native_renewal_survives_plugin_disable(self):
        p=packet();certificate=t.certificate_manifest(p,config())
        self.assertEqual(certificate['spec']['dnsNames'],['50.cms.example.com'])
        self.assertEqual(certificate['spec']['issuerRef'],{'name':'cms-acme','kind':'ClusterIssuer','group':'cert-manager.io'})
        self.assertEqual(certificate['spec']['secretName'],'app-40-tls')
        self.assertEqual(certificate['spec']['secretTemplate']['labels'],t.metadata(p,'app-40-tls')['labels'])
        self.assertNotIn('ownerReferences',certificate['metadata'])
        self.assertEqual(certificate['spec']['privateKey']['rotationPolicy'],'Always')
        self.assertFalse(hasattr(t,'maintain'))

    def test_publication_waits_for_certificate_and_rejects_foreign_resources(self):
        for scenario in ['pending','foreign-certificate','foreign-secret','replaced-certificate','ready']:
            with self.subTest(scenario=scenario):
                p=packet();deployment,_=t.e.manifests(p);deployment['metadata'].update(uid='deploy',generation=1);deployment['status']={'availableReplicas':1,'observedGeneration':1}
                cert=t.certificate_manifest(p,config());cert['metadata'].update(uid='cert',generation=1);cert['status']={'conditions':[{'type':'Ready','status':'False' if scenario=='pending' else 'True','observedGeneration':1}]}
                ingress=t.publication_manifest(p,config());ingress['metadata'].update(uid='ingress')
                secret={'type':'kubernetes.io/tls','metadata':{**t.metadata(p,'app-40-tls'),'annotations':{'cert-manager.io/certificate-name':'app-40-tls'}}}
                if scenario=='foreign-certificate':cert['metadata']['labels']['webdock.dev/provisioning-id']='999'
                if scenario=='foreign-secret':secret['metadata']['labels']['webdock.dev/provisioning-id']='999'
                if scenario=='replaced-certificate':p['certificateUID']='previous-cert'
                def get(kind,name,namespace=None):return {'deployment':deployment,'Certificate':cert,'certificate':cert,'ingress':ingress,'secret':secret,'job':None}.get(kind)
                with patch.object(t.e,'get',side_effect=get),patch.object(t.e,'kubectl',return_value='{"items":[]}'),patch.object(t.e,'apply') as apply,patch.object(t.e,'prepare_namespace'),patch.object(t,'tls_health') as health:
                    if scenario.startswith('foreign') or scenario=='replaced-certificate':
                        with self.assertRaises(Exception):t.publish(p,config(),lambda **value:None)
                        health.assert_not_called()
                    else:
                        proof=t.publish(p,config(),lambda **value:None)
                        if scenario=='pending':self.assertIsNone(proof);health.assert_not_called()
                        else:self.assertEqual(proof['certificateUID'],'cert');health.assert_called_once()
                    self.assertFalse(any(call.args[0]['kind']=='Secret' for call in apply.call_args_list))

    def test_owned_certificate_budget_is_exact_and_survives_ordinary_restart(self):
        p=packet();namespace={'metadata':{'name':p['namespace'],'labels':{**t.e.labels(p,False),'webdock.dev/certificate-budget':'1'}}}
        cert=t.certificate_manifest(p,config());objects=[]
        with patch.object(t,'approved',return_value=config()),patch.object(t.e,'get',return_value=namespace),patch.object(t.e,'kubectl',return_value=t.json.dumps({'items':[cert]})),patch.object(t.e,'apply',side_effect=lambda obj,*args,**kwargs:objects.append(obj)):
            t.e.prepare_namespace(p,p['namespace'])
        quota=next(o for o in objects if o['kind']=='ResourceQuota')['spec']['hard']
        self.assertEqual(quota['requests.cpu'],'275m');self.assertEqual(quota['limits.cpu'],'450m')
        self.assertEqual(quota['requests.memory'],str(p['quota']['memoryBytes']+33554432));self.assertEqual(quota['limits.memory'],str(p['quota']['memoryBytes']+67108864))
        self.assertEqual(quota['services'],'2')
        cert['spec']['issuerRef']['name']='foreign'
        with patch.object(t,'approved',return_value=config()),patch.object(t.e,'kubectl',return_value=t.json.dumps({'items':[cert]})),self.assertRaises(ValueError):t.certificate_budget(p,namespace)

    def test_normal_namespace_has_no_certificate_entitlement_or_lookup(self):
        with patch.object(t,'approved') as approved,patch.object(t.e,'kubectl') as kubectl:
            self.assertFalse(t.certificate_budget(packet(),{'metadata':{'labels':{}}}))
        approved.assert_not_called();kubectl.assert_not_called()

    def test_health_gateway_convergence_503_then_200_is_pending_not_failure(self):
        from unittest.mock import MagicMock
        p=packet();deployment,_=t.e.manifests(p);deployment['metadata'].update(uid='deploy',generation=1);deployment['status']={'availableReplicas':1,'observedGeneration':1}
        cert=t.certificate_manifest(p,config());cert['metadata'].update(uid='cert',generation=1);cert['status']={'conditions':[{'type':'Ready','status':'True','observedGeneration':1}]}
        ingress=t.publication_manifest(p,config());ingress['metadata']['uid']='ingress'
        secret={'type':'kubernetes.io/tls','metadata':{**t.metadata(p,'app-40-tls'),'annotations':{'cert-manager.io/certificate-name':'app-40-tls'}}}
        def get(kind,name,namespace=None):return {'deployment':deployment,'Certificate':cert,'ingress':ingress,'secret':secret}.get(kind)
        unavailable=MagicMock(status=503);unavailable.read.return_value=b'upstream unavailable'
        ready=MagicMock(status=200);ready.read.return_value=b'{"status":"ok","instance":"50","project":"20","schema":"app"}'
        foreign=MagicMock(status=200);foreign.read.return_value=b'{"status":"ok","instance":"other","project":"20","schema":"app"}'
        connection=MagicMock();connection.getresponse.side_effect=[unavailable,ready,foreign]
        with patch.object(t.e,'get',side_effect=get),patch.object(t.e,'kubectl',return_value='{"items":[]}'),patch.object(t.e,'apply'),patch.object(t.e,'prepare_namespace'),patch.object(t.socket,'getaddrinfo',return_value=[(2,1,6,'',('192.0.2.1',443))]),patch.object(t.socket,'create_connection'),patch.object(t.http.client,'HTTPSConnection',return_value=connection):
            self.assertIsNone(t.publish(p,config(),lambda **value:None))
            self.assertEqual(t.publish(p,config(),lambda **value:None)['instanceID'],'50')
            with self.assertRaisesRegex(ValueError,'identity differs'):t.publish(p,config(),lambda **value:None)
            connection._context.wrap_socket.side_effect=t.ssl.SSLCertVerificationError('wrong hostname')
            with self.assertRaisesRegex(ValueError,'hostname or chain'):t.publish(p,config(),lambda **value:None)
        self.assertEqual(connection.request.call_count,3)

    def test_http01_solver_route_requires_exact_certificate_owner_chain(self):
        p=packet();cert=t.certificate_manifest(p,config());cert['metadata']['uid']='cert'
        def owner(kind,name,uid):return [{'apiVersion':'acme.cert-manager.io/v1' if kind in ['Challenge','Order'] else 'cert-manager.io/v1','kind':kind,'name':name,'uid':uid,'controller':True}]
        route={'metadata':{'name':'solver','namespace':p['namespace'],'labels':{'acme.cert-manager.io/http01-solver':'true'},'ownerReferences':owner('Challenge','challenge','challenge')},'spec':{'rules':[{'host':'50.cms.example.com','http':{'paths':[{'path':'/.well-known/acme-challenge/testtoken'}]}}]}}
        challenge={'metadata':{'uid':'challenge','ownerReferences':owner('Order','order','order')},'spec':{'dnsName':'50.cms.example.com','type':'HTTP-01','issuerRef':cert['spec']['issuerRef']}}
        order={'metadata':{'uid':'order','ownerReferences':owner('CertificateRequest','request','request')}}
        request={'metadata':{'uid':'request','ownerReferences':owner('Certificate','app-40-tls','cert')}}
        with patch.object(t.e,'get',side_effect=lambda kind,*args:{'Challenge':challenge,'Order':order,'CertificateRequest':request}.get(kind)):
            self.assertTrue(t.owned_solver_route(route,p,config(),cert))
            request['metadata']['ownerReferences'][0]['uid']='foreign'
            self.assertFalse(t.owned_solver_route(route,p,config(),cert))

if __name__=='__main__':unittest.main()
