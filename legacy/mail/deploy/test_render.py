import unittest
from render import render

class DeploymentTest(unittest.TestCase):
    def resources(self):
        return render('webdock.local/mail-api@sha256:'+'a'*64,'webdock.local/mail-gateway@sha256:'+'b'*64)

    def test_only_public_paths_reach_ingress(self):
        resources=self.resources()
        ingress=next(r for r in resources if r['kind']=='Ingress')
        paths=[p['path'] for r in ingress['spec']['rules'] for p in r['http']['paths']]
        self.assertEqual(paths,['/v1/emails','/session','/s/'])
        self.assertNotIn('/',paths)
        self.assertFalse(any(p.startswith('/internal') for p in paths))

    def test_secret_and_volume_separation(self):
        resources=self.resources()
        deployment=next(r for r in resources if r['kind']=='Deployment')
        self.assertEqual(deployment['spec']['replicas'],1)
        self.assertEqual(deployment['spec']['strategy']['type'],'Recreate')
        pod=deployment['spec']['template']['spec']
        self.assertFalse(pod['automountServiceAccountToken'])
        api,gateway=pod['containers']
        self.assertNotIn('volumeMounts',gateway)
        self.assertEqual(api['volumeMounts'][0]['name'],'payloads')
        self.assertNotIn('MAIL_SEND_WORKER_SECRET',[e['name'] for e in gateway['env']])
        self.assertNotIn('MAIL_ENCRYPTION_KEY',[e['name'] for e in gateway['env']])
        self.assertFalse(any(r['kind'] in ('Secret','PersistentVolume','PersistentVolumeClaim') for r in resources))
        for container in pod['containers']:
            self.assertTrue(container['securityContext']['readOnlyRootFilesystem'])
            self.assertEqual(container['securityContext']['capabilities']['drop'],['ALL'])
        network=next(r for r in resources if r['kind']=='NetworkPolicy' and r['metadata']['name']=='mail-network')
        self.assertFalse(any(p['port'] in (25,465,587) for rule in network['spec']['egress'] for p in rule['ports']))

    def test_mutable_images_rejected(self):
        with self.assertRaises(ValueError):render('mail:latest','gateway:latest')

if __name__=='__main__':unittest.main()
