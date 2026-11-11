"""Pure offline profile checks; real Caddy transport has a separate local qualifier."""
import copy
import importlib.util
import json
from pathlib import Path
import unittest

ROOT=Path(__file__).resolve().parents[1]


def load():
    path=ROOT/'render.py'
    spec=importlib.util.spec_from_file_location('gateway_profile',path)
    module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module);return module


def config():
    c=json.loads((ROOT/'profile.example.json').read_text())
    c.update(enabled=True,namespace='owned-platform',caddyImage='docker.io/library/caddy@sha256:'+'a'*64,gatewayOrigin='https://edge.example.test',coreUpstream='http://core.owned-platform.svc.cluster.local:3130',consoleOrigin='https://console.example.test',studioOrigin='https://studio.example.test',authOrigin='https://auth.example.test',apiOrigin='https://api.example.test',tlsSecretName='owned-edge-tls',ingressClassName='traefik',ingressControllerNamespace='kube-system',ingressControllerPodLabels={'app.kubernetes.io/name':'traefik'},coreNamespace='owned-platform',corePodLabels={'app.kubernetes.io/name':'owned-core'})
    return c


class Gateway(unittest.TestCase):
    def test_realtime_csp_is_optional_and_pins_one_verified_neon_origin(self):
        m=self.module();c=config()
        realtime='wss://ep-owned.rt.c-7.eu-central-1.aws.neon.tech/'
        normal=m.vercel_output(c,'console')['routes'][0]['headers']['content-security-policy']
        self.assertNotIn('neon.tech',normal)
        policy=m.vercel_output(c,'console',realtime_url=realtime)['routes'][0]['headers']['content-security-policy']
        self.assertIn('wss://ep-owned.rt.c-7.eu-central-1.aws.neon.tech',policy)
        self.assertNotIn('*.neon.tech',policy)
        for bad in ['wss://*.neon.tech/','wss://foreign.test/','https://ep.neon.tech/','wss://ep.neon.tech/?token=x']:
            with self.subTest(bad=bad),self.assertRaises(ValueError):m.vercel_output(c,'console',realtime_url=bad)

    def test_native_service_navigation_aliases_do_not_claim_protocol_authority(self):
        c=config();c['coreUpstream']='http://10.90.0.20:3130'
        text=self.module().native_https(c,navigation_aliases={'https://docs.spitzli.cloud':'https://docs.webdock.dev'})
        block=text.split('docs.spitzli.cloud {',1)[1]
        self.assertIn('method GET HEAD',block)
        self.assertIn('redir https://docs.webdock.dev{uri} 307',block)
        self.assertIn('/api/*',block)
        self.assertIn('/.well-known/*',block)
        self.assertNotIn('reverse_proxy',block)
        self.assertIn('respond 404',block)
    def test_legacy_service_navigation_can_point_forward_to_spitzli(self):
        c=config();c['coreUpstream']='http://10.90.0.20:3130'
        text=self.module().native_https(c,navigation_aliases={'https://docs.webdock.dev':'https://docs.spitzli.cloud'})
        block=text.split('docs.webdock.dev {',1)[1]
        self.assertIn('redir https://docs.spitzli.cloud{uri} 307',block)
        self.assertIn('method GET HEAD',block)
        self.assertNotIn('reverse_proxy',block)

    def test_navigation_aliases_require_reviewed_destinations_and_explicit_activation(self):
        c=config();c['coreUpstream']='http://10.90.0.20:3130'
        for mapping in [{'https://docs.spitzli.cloud':'https://unexpected.example.test'}, {'https://api.example.test':'https://docs.webdock.dev'}]:
            with self.assertRaises(ValueError):self.module().native_https(c,navigation_aliases=mapping)
        with self.assertRaises(ValueError):self.module().native_https(c,api_public=False,navigation_aliases={'https://docs.spitzli.cloud':'https://docs.webdock.dev'})

    def test_static_frontend_keeps_studio_mcp_and_browser_apis_out_of_spa_fallback(self):
        result=self.module().vercel_output(config(),'console','https://database.example.test')
        self.assertEqual(result['version'],3)
        routes=result['routes']
        mcp=next(r for r in routes if r.get('src')=='^/api/mcp$')
        self.assertEqual(mcp['dest'],'https://edge.example.test/__studio/api/mcp')
        self.assertEqual(mcp['has'],[{'type':'host','value':{'eq':'studio.example.test'}}])
        self.assertEqual(mcp['headers']['cache-control'],'private, no-store')
        self.assertTrue(any(r.get('status')==404 and 'api' in r.get('src','') for r in routes))
        self.assertEqual(routes[-1]['dest'],'/index.html')
        self.assertIn("frame-ancestors 'none'",routes[0]['headers']['content-security-policy'])
        account=self.module().vercel_output(config(),'account')
        self.assertTrue(any(r.get('dest','').startswith('https://edge.example.test/__account/api/auth/') for r in account['routes']))
    def test_console_mcp_uses_existing_studio_bearer_authority(self):
        routes=self.module().vercel_output(config(),'console')['routes']
        canonical=[r for r in routes if r.get('src')=='^/api/mcp$' and not r.get('has')]
        self.assertEqual(len(canonical),1,'the canonical Console endpoint must not fall through to404')
        self.assertEqual(canonical[0]['dest'],'https://edge.example.test/__studio/api/mcp')
        self.assertEqual(canonical[0]['headers']['cache-control'],'private, no-store')
        # Cookie-based Console and Auth endpoints retain their own Host boundary.
        for path in ['^/api/console/','^/api/sso/login$']:
            self.assertFalse(any(r.get('src','').startswith(path) and not r.get('has') and '/__studio/' in r.get('dest','') for r in routes))
        account=self.module().vercel_output(config(),'account')['routes']
        self.assertFalse(any(r.get('src')=='^/api/mcp$' for r in account))

    def test_native_https_keeps_prefix_authority_and_denies_public_worker_routes(self):
        c=config();c['coreUpstream']='http://10.90.0.20:3130'
        text=self.module().native_https(c)
        self.assertIn('edge.example.test {',text)
        self.assertIn('api.example.test {',text)
        self.assertNotIn('auto_https off',text)
        self.assertIn('header_up Host auth.example.test',text)
        self.assertIn('@private_worker path /api/hosting/git/worker*',text)
        self.assertIn('respond @private_worker 404',text)
        self.assertIn('header_up -Forwarded',text)
        self.assertNotIn('trusted_proxies',text)
        self.assertNotIn('header_up Origin',text)
    def test_new_navigation_alias_redirects_only_to_its_fixed_canonical_surface(self):
        m=self.module();c=config()
        result=m.vercel_output(c,'account',navigation_aliases=['https://auth.spitzli.cloud'])
        rules=[r for r in result['routes'] if r.get('has')==[{'type':'host','value':{'eq':'auth.spitzli.cloud'}}]]
        self.assertEqual(len(rules),2)
        self.assertEqual(rules[0]['status'],404)
        self.assertIn('api',rules[0]['src']);self.assertIn('well-known',rules[0]['src'])
        self.assertEqual(rules[1]['status'],307)
        self.assertEqual(rules[1]['headers']['Location'],'https://auth.example.test/$1')
        for alias in ['https://auth.example.test','https://console.example.test','https://*.spitzli.cloud','https://auth.spitzli.cloud/evil','https://auth.spitzli.cloud\\x']:
            with self.subTest(alias=alias),self.assertRaises(ValueError):m.vercel_output(c,'account',navigation_aliases=[alias])
    def test_legacy_account_navigation_uses_new_origin_without_redirecting_protocol_posts(self):
        m=self.module();c=config();old=c['authOrigin'];c['authOrigin']='https://auth.spitzli.cloud'
        result=m.vercel_output(c,'account',compatibility_aliases=[old])
        rules=[r for r in result['routes'] if r.get('has')==[{'type':'host','value':{'eq':'auth.example.test'}}]]
        self.assertEqual(len(rules),2)
        policy=result['routes'][0]['headers']['content-security-policy']
        self.assertIn('https://auth.example.test',policy)
        self.assertIn('https://auth.spitzli.cloud',policy)
        for rule in rules:
            self.assertEqual(rule['methods'],['GET','HEAD'])
            self.assertEqual(rule['status'],307)
            self.assertEqual(rule['headers']['Location'],'https://auth.spitzli.cloud/$1')
        self.assertEqual(rules[0]['src'],'^/(api/auth/oauth2/authorize)$')
        import re
        for path in ['api/auth/oauth2/token','api/auth/get-session','.well-known/openid-configuration','.well-known/webauthn','v1/auth/session','assets/worker.js']:
            self.assertIsNone(re.fullmatch(rules[1]['src'],'/'+path))
        self.assertTrue(any(r.get('dest','').startswith('https://edge.example.test/__account/api/auth/') for r in result['routes']))

    def test_legacy_studio_navigation_keeps_mcp_authority_and_rejects_alias_overlap(self):
        m=self.module();c=config()
        result=m.vercel_output(c,'console',compatibility_aliases=[c['studioOrigin']])
        nav=next(r for r in result['routes'] if r.get('status')==307)
        self.assertEqual(nav['headers']['Location'],'https://console.example.test/$1')
        self.assertEqual(nav['methods'],['GET','HEAD'])
        self.assertTrue(any(r.get('dest')=='https://edge.example.test/__studio/api/mcp' for r in result['routes']))
        for bad in [c['consoleOrigin'],c['apiOrigin'],c['authOrigin'],c['gatewayOrigin'],'https://*.example.test','http://old.example.test','https://old.example.test/path']:
            with self.subTest(bad=bad),self.assertRaises(ValueError):m.vercel_output(c,'console',compatibility_aliases=[bad])
        with self.assertRaises(ValueError):m.vercel_output(c,'console',navigation_aliases=['https://old.example.test'],compatibility_aliases=['https://old.example.test'])

    def test_api_root_navigation_can_use_an_explicit_alias_without_moving_protocol_paths(self):
        c=config();c['coreUpstream']='http://10.90.0.20:3130'
        text=self.module().native_https(c,api_aliases=['https://api.spitzli.cloud'],api_navigation_target='https://api.spitzli.cloud')
        old=text.split('api.example.test {',1)[1].split('api.spitzli.cloud {',1)[0]
        new=text.split('api.spitzli.cloud {',1)[1]
        self.assertIn('path /',old);self.assertIn('method GET HEAD',old)
        self.assertIn('redir @root_navigation https://api.spitzli.cloud{uri} 307',old)
        self.assertNotIn('root_navigation',new)
        self.assertIn('/v1/*',old)
        with self.assertRaises(ValueError):self.module().native_https(c,api_navigation_target='https://unreviewed.example.test')

    def test_public_api_alias_preserves_original_authority_and_worker_denial(self):
        m=self.module();c=config()
        text=m.native_https(c,api_aliases=['https://api.spitzli.cloud'])
        self.assertIn('api.spitzli.cloud {',text)
        self.assertEqual(text.count('header_up Host api.example.test'),3)
        self.assertEqual(text.count('respond @private_worker 404'),3)
        self.assertNotIn('header_up Host api.spitzli.cloud',text)
        for alias in ['https://auth.example.test','https://api.example.test','http://api.spitzli.cloud','https://api.spitzli.cloud/extra']:
            with self.subTest(alias=alias),self.assertRaises(ValueError):m.native_https(c,api_aliases=[alias])

    def module(self):
        self.assertTrue((ROOT/'render.py').is_file(),'disabled strict-host gateway renderer is missing')
        return load()

    def test_default_disabled_emits_no_deployment_or_live_routes(self):
        m=self.module();result=m.render(json.loads((ROOT/'profile.example.json').read_text()))
        self.assertFalse(result['enabled']);self.assertEqual(result['resources'],[])
        self.assertNotIn('Caddyfile',result)

    def test_bad_origins_paths_pins_and_header_injection_are_refused(self):
        m=self.module()
        for key,value in [('consoleOrigin','https://console.example.test/path'),('authOrigin','https://user:secret@auth.example.test'),('gatewayOrigin','https://edge.example.test/?x=1'),('studioOrigin','https://studio.example.test\r\nX-Pwn:yes'),('apiOrigin','http://api.example.test'),('coreUpstream','http://169.254.169.254:80'),('coreUpstream','http://core:3130/private'),('coreUpstream','https://core:3130'),('caddyImage','caddy:latest'),('tlsSecretName','../foreign'),('listenPort',80)]:
            c=config();c[key]=value
            with self.subTest(key=key,value=value),self.assertRaises(ValueError):m.render(c)
        c=config();c['dbPassword']='must-not-accept'
        with self.assertRaises(ValueError):m.render(c)

    def test_only_fixed_prefixes_and_canonical_hosts_are_rendered(self):
        m=self.module();result=m.render(config());text=result['Caddyfile']
        for surface,host in [('console','console.example.test'),('studio','studio.example.test'),('account','auth.example.test'),('workspace','api.example.test')]:
            self.assertIn('uri strip_prefix /__'+surface,text)
            self.assertIn('header_up Host '+host,text)
        for header in ['Forwarded','X-Forwarded-*','X-Vercel-*','Connection','Proxy-Authorization','Upgrade']:
            self.assertIn('header_up -'+header,text)
        self.assertIn('admin off',text);self.assertIn('auto_https off',text)
        self.assertIn('storage file_system',text);self.assertIn('root /tmp/caddy-data',text)
        self.assertIn('flush_interval -1',text);self.assertIn('respond 404',text)
        self.assertIn('/api/hosting/git/worker',text)
        self.assertNotIn('header_down Location',text)
        self.assertNotIn('header_up Origin',text)
        self.assertNotIn('trusted_proxies',text)

    def test_gateway_pod_has_no_authority_and_tls_stays_at_ingress(self):
        result=self.module().render(config())
        pod=next(x for x in result['resources'] if x['kind']=='Deployment')['spec']['template']['spec']
        self.assertFalse(pod['automountServiceAccountToken']);self.assertFalse(pod['enableServiceLinks'])
        self.assertEqual(pod['securityContext']['fsGroup'],10001)
        self.assertNotIn('hostNetwork',pod)
        container=pod['containers'][0];security=container['securityContext']
        self.assertTrue(security['readOnlyRootFilesystem']);self.assertFalse(security['allowPrivilegeEscalation']);self.assertEqual(security['capabilities']['drop'],['ALL'])
        self.assertNotIn('envFrom',container);self.assertNotIn('secret',str(pod['volumes']))
        ingress=next(x for x in result['resources'] if x['kind']=='Ingress')
        self.assertEqual(ingress['spec']['tls'],[{'hosts':['edge.example.test'],'secretName':'owned-edge-tls'}])

    def test_rewrites_keep_frontend_paths_and_no_well_known_proxy_claim(self):
        result=self.module().render(config())
        for surface,review in result['vercel'].items():
            for rule in review['rewrites']:
                self.assertTrue(rule['destination'].startswith('https://edge.example.test/__'+surface+'/'))
                self.assertFalse(rule['source'].startswith('/.well-known'))
            self.assertTrue(any(x['source']=='/console-config.json' for x in review['rewrites']))
        self.assertIn('wellKnown',result['qualificationGates'])


if __name__=='__main__':unittest.main()
