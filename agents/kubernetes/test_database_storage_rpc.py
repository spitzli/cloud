import io
import json
import os
import unittest
from unittest.mock import patch
from datetime import datetime, timezone, timedelta
import database_storage_rpc as rpc


def packet():
    return {'version':1,'installationID':'123','clusterID':'456','revision':1,
            'node':'node01','poolID':'9cd1dd12-6d6f-4478-96ae-d2cae036269a',
            'volumeBytes':67108864,'leaseUntil':(datetime.now(timezone.utc)+timedelta(minutes=2)).isoformat()}


class DatabaseStorageRPCTests(unittest.TestCase):
    def invoke(self, value, *, response=None, failure=None, root=True, environment=None):
        raw=value if isinstance(value,bytes) else json.dumps(value).encode()+b'\n'
        output=io.BytesIO()
        stdin=io.TextIOWrapper(io.BytesIO(raw))
        stdout=io.TextIOWrapper(output,write_through=True)
        with patch.object(rpc.os,'geteuid',return_value=0 if root else 1000), \
             patch.dict(os.environ,environment or {},clear=True), \
             patch.object(rpc.sys,'stdin',stdin),patch.object(rpc.sys,'stdout',stdout), \
             patch.object(rpc,'request',return_value=response,side_effect=failure) as request:
            status=rpc.main()
            result=output.getvalue()
        return status,result,request

    def test_only_explicit_database_actions_forward_to_fixed_socket_and_keep_result(self):
        result={'nested':{'node':'node01','allocations':[{'installationID':'123'}]},'ready':False}
        for action in ['database.capability','database.ensure','database.state','database.retained','database.purge']:
            value=['node01'] if action=='database.capability' else packet()
            with self.subTest(action=action):
                status,output,request=self.invoke({'action':action,'value':value},response=result)
                self.assertEqual(status,0)
                self.assertEqual(json.loads(output),{'ok':True,'value':result})
                request.assert_called_once_with(action,value,socket_path='/run/webdock-postgres-storage/control.sock')

    def test_nonroot_unknown_malformed_and_unbounded_requests_fail_nonempty_without_helper(self):
        cases=[(b'not-json\n',True),(b'{}',True),(b'x'*4097+b'\n',True),
               ({'action':'inventory','value':None},True),({'action':'database.capability','value':{}},True),
               ({'action':'database.capability','value':['node01']*201},True),
               ({'action':'database.state','value':{}},True),
               ({'action':'database.capability','value':['node01'],'socket':'/tmp/evil'},True),
               ({'action':'database.capability','value':['node01']},False)]
        for value,root in cases:
            with self.subTest(value=str(value)[:80],root=root):
                status,output,request=self.invoke(value,root=root)
                self.assertNotEqual(status,0)
                self.assertEqual(json.loads(output),{'ok':False,'value':None})
                request.assert_not_called()

    def test_socket_environment_cannot_redirect_or_expose_inline_secrets(self):
        secret='private-inline-secret-do-not-print'
        status,output,request=self.invoke({'action':'database.capability','value':['node01']},environment={'SPITZLI_STORAGE_SOCKET':'/tmp/'+secret})
        self.assertNotEqual(status,0)
        self.assertEqual(json.loads(output),{'ok':False,'value':None})
        self.assertNotIn(secret.encode(),output)
        request.assert_not_called()
        status,output,request=self.invoke({'action':'database.capability','value':['node01']},environment={'SPITZLI_STORAGE_SOCKET':'/run/webdock-postgres-storage/control.sock'},response={'ready':True})
        self.assertEqual(status,0)
        request.assert_called_once()

    def test_unavailable_helper_is_empty_eof_but_protocol_or_identity_failure_is_not(self):
        for failure in [FileNotFoundError('secret-path'),ConnectionRefusedError('secret-path'),ConnectionError('peer closed'),TimeoutError('timeout')]:
            with self.subTest(failure=type(failure).__name__):
                status,output,_=self.invoke({'action':'database.capability','value':['node01']},failure=failure)
                self.assertNotEqual(status,0)
                self.assertEqual(output,b'')
        for failure in [ValueError('wrong pool secret'),json.JSONDecodeError('bad helper response','secret',0)]:
            status,output,_=self.invoke({'action':'database.capability','value':['node01']},failure=failure)
            self.assertNotEqual(status,0)
            self.assertEqual(json.loads(output),{'ok':False,'value':None})
            self.assertNotIn(b'secret',output)

    def test_output_is_bounded_before_emission_and_non_json_values_fail_closed(self):
        for response in ['x'*8192,float('nan'),object()]:
            status,output,_=self.invoke({'action':'database.capability','value':['node01']},response=response)
            self.assertNotEqual(status,0)
            self.assertEqual(json.loads(output),{'ok':False,'value':None})
            self.assertLessEqual(len(output),8192)


if __name__=='__main__':unittest.main()
