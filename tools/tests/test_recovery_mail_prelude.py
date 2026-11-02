import ast, copy, importlib.util, pathlib, unittest

path=pathlib.Path(__file__).with_name('recovery-admission-component.py')
spec=importlib.util.spec_from_file_location('mail_prelude_runner',path)
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)

class MailPreludeTests(unittest.TestCase):
 def proofs(self):
  marker={'version':1,'scope':'genuine-mail-prelude-only-v1','preludeOnly':True,'passed':True,'checkpoints':5,'samePinnedUIDAndProcess':True,'mailActions':0,'actionPrefixQualified':False,'wholeMailQualified':False,'backup':False,'sourceUnavailable':False,'coldRestore':False}
  process={'pid':123,'alive':True,'startTime':456,'uid':65532,'networkNamespaceInode':789,'cgroupSHA256':'a'*64}
  containers=[{'kind':kind,'index':0,'restartPolicy':'absent','restartRulesPresent':False,'restartRulesCount':None,'state':'terminated' if kind=='init' else 'running','ready':True,'restarts':0,'exitCode':0 if kind=='init' else None,'signal':None,'startedAtMs':None,'finishedAtMs':None} for kind in ('init','runtime')]
  forward={'pid':222,'category':'not_exited','exitCode':None,'stdout':{'bytes':0,'sha256':m.hashlib.sha256(b'').hexdigest(),'hashComplete':True,'category':'empty'},'stderr':{'bytes':0,'sha256':m.hashlib.sha256(b'').hexdigest(),'hashComplete':True,'category':'empty'}}
  rows=[{'scope':'genuine-mail-prelude-checkpoint-v1','stage':stage,'podUID':'11111111-1111-1111-1111-111111111111','containerID':'b'*64,'podQuerySHA256':'c'*64,'sameProcess':True,'beforeProcess':copy.deepcopy(process),'afterProcess':copy.deepcopy(process),'lifetime':{'category':'actual_pod_lifetime','restartPolicy':'Always','deleting':False,'deletionAtMs':None,'phase':'Running','disruptionTarget':None,'containers':copy.deepcopy(containers)},'forward':copy.deepcopy(forward),'passed':True} for stage in ('installed','gateway401','enabled','before_namespace_entry','smtp_ready')]
  trace={'scope':'genuine-signed-mail-actions-v1','roles':[{'role':role,'action':action,'entered':0,'completed':0,'httpStatus':None,'publicCode':'missing'} for role,action in m.MAIL_INVOKE_ROLES.items()],'allRolesCompleted200':False}
  return marker,rows,trace
 def test_prelude_scope_stays_distinct_from_actions_and_whole(self):
  proof=m.validate_mail_prelude(*self.proofs())
  self.assertTrue(proof['preludeOnly']);self.assertFalse(proof['wholeMailQualified']);self.assertFalse(proof['actionPrefixQualified'])
  with self.assertRaises(ValueError):m.validate_mail_invoke_prefix(self.proofs()[0],self.proofs()[2])
 def test_each_checkpoint_identity_policy_process_stream_and_action_poison_denies(self):
  changes=[('uid',lambda r:r[2].update(podUID='22222222-2222-2222-2222-222222222222')),('policy',lambda r:r[2]['lifetime'].update(restartPolicy='Never')),('disruption',lambda r:r[2]['lifetime'].update(disruptionTarget={'status':'True','reason':'TerminationByKubelet'})),('process',lambda r:r[2]['afterProcess'].update(startTime=999)),('forward',lambda r:r[2]['forward'].update(category='exited',exitCode=1)),('partial',lambda r:r[2]['forward']['stdout'].update(hashComplete=False)),('rule',lambda r:r[2]['lifetime']['containers'][1].update(restartRulesPresent=True,restartRulesCount=0)),('private',lambda r:r[2].update(private='poison'))]
  for label,change in changes:
   with self.subTest(label=label):
    marker,rows,trace=self.proofs();change(rows)
    with self.assertRaises(ValueError):m.validate_mail_prelude(marker,rows,trace)
  marker,rows,trace=self.proofs();trace['roles'][0]['entered']=1
  with self.assertRaises(ValueError):m.validate_mail_prelude(marker,rows,trace)
  marker,rows,trace=self.proofs();marker['wholeMailQualified']=True
  with self.assertRaises(ValueError):m.validate_mail_prelude(marker,rows,trace)
 def test_order_missing_bool_and_replaced_pid_fail(self):
  for edit in [lambda r:r.reverse(),lambda r:r.pop(),lambda r:r[0]['beforeProcess'].update(pid=True),lambda r:r[3]['beforeProcess'].update(pid=999),lambda r:r[2]['lifetime']['containers'][1].update(signal=15)]:
   marker,rows,trace=self.proofs();edit(rows)
   with self.assertRaises(ValueError):m.validate_mail_prelude(marker,rows,trace)
 def test_exact_worker_lifetime_grammar_never_keeps_private_lines(self):
  scope={};exec(compile(m.MAIL_STATE_CAPTURE,'owned-mail-state','exec'),{'__name__':'owned_mail_state'},scope)
  # The real generated collector functions need the same globals dictionary.
  scope={'__name__':'owned_mail_state'};exec(compile(m.MAIL_STATE_CAPTURE,'owned-mail-state','exec'),scope)
  allowed=[b'mail_lifetime event='+x for x in (b'SIGTERM',b'SIGINT',b'signal_api_error',b'signal_stream_closed',b'server_ok',b'server_err',b'requested_shutdown',b'dispatcher_stopped')]
  poison=[b'private-password',b'mail_lifetime event=SIGTERM private-password',b'prefix mail_lifetime event=SIGTERM',b'mail_lifetime event=unknown']
  result=scope['worker_log_projection'](b'\n'.join(allowed+poison)+b'\n',b'',0,'complete')
  self.assertEqual(result['lines'],[x.decode() for x in allowed]);self.assertEqual(result['discardedLineCount'],len(poison));self.assertNotIn('private-password',str(result))

if __name__=='__main__':unittest.main()
