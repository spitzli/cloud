"""Run the actual generated daemon launch lines with private public stubs only."""
import ast,importlib.util,json,os,pathlib,re,subprocess,tempfile,unittest
P=pathlib.Path
path=P(__file__).with_name('recovery-admission-component.py');spec=importlib.util.spec_from_file_location('mask_runner',path);m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
class DaemonMasks(unittest.TestCase):
 def test_actual_launch_preserves_parent_engine_private_files_pid_and_argv(self):
  raw=next(n.value for n in ast.walk(ast.parse(path.read_bytes())) if isinstance(n,ast.Constant) and isinstance(n.value,str) and n.value.startswith('#!/bin/bash\n') and '__OWNED_NESTING__' in n.value)
  k3s=next(line for line in raw.splitlines() if '/usr/local/bin/k3s server --data-dir=' in line)
  engine=next(line for line in raw.splitlines() if line.startswith('dockerd --host='))
  with tempfile.TemporaryDirectory() as tmp:
   root=P(tmp);(root/'diagnostics').mkdir();stub=root/'daemon';stub.write_text('#!/usr/bin/python3\nimport json,os,pathlib,re,sys\nv=re.search(r"^Umask:\\s+([0-7]{4})$",pathlib.Path("/proc/self/status").read_text(),re.M)[1]\npathlib.Path(os.environ["MASK_RECEIPT"]).write_text(json.dumps({"pid":os.getpid(),"mask":v,"argv":sys.argv[1:]}))\n');stub.chmod(0o700)
   engine=engine.replace('dockerd ',str(stub)+' ',1).replace('/diagnostics/',str(root/'diagnostics')+'/')
   k3s=k3s.replace('/usr/local/bin/k3s ',str(stub)+' ',1).replace('/diagnostics/',str(root/'diagnostics')+'/')
   script='set -eu\numask 077\n: >"'+str(root/'private')+'"\nexport MASK_RECEIPT="'+str(root/'engine.json')+'"\n'+engine+'\ne=$!\nwait "$e"\nexport MASK_RECEIPT="'+str(root/'k3s.json')+'"\n'+k3s+'\nk=$!\nwait "$k"\nprintf "%s %s %s" "$e" "$k" "$(umask)"\n'
   q=subprocess.run(['/bin/bash','-c',script],capture_output=True,timeout=5,env={**os.environ,'HOSTNAME':'public-node','OWNED_PAUSE':'public-reference'});self.assertEqual(q.returncode,0);e,k,parent=q.stdout.decode().split();er=json.loads((root/'engine.json').read_bytes());kr=json.loads((root/'k3s.json').read_bytes())
   self.assertEqual(er['pid'],int(e));self.assertEqual(kr['pid'],int(k));self.assertEqual(parent,'0077');self.assertEqual(er['mask'],'0077');self.assertEqual(kr['mask'],'0022');self.assertEqual((root/'private').stat().st_mode&0o777,0o600)
   self.assertEqual(kr['argv'][0],'server');self.assertIn('--pause-image=public-reference',kr['argv']);self.assertIn('--feature',er['argv']);self.assertIn('containerd-snapshotter=true',er['argv'])
 def test_generated_mail_precore_capture_records_measured_effective_engine_mask(self):
  raw=next(n.value for n in ast.walk(ast.parse(path.read_bytes())) if isinstance(n,ast.Constant) and isinstance(n.value,str) and n.value.startswith('#!/bin/bash\n') and '__OWNED_NESTING__' in n.value)
  generated=m.mail_preflight(raw);self.assertIn('/diagnostics/daemon-os-contract.json',generated);self.assertLess(generated.index('OWNED_DAEMON_OS_CAPTURE\n'),generated.index('printf nested_tool_boundary_ready'))
  compile(m.MAIL_DAEMON_OS_CAPTURE,'fixed-mask-capture','exec');self.assertIn("('parent','0077'),('engine','0022'),('k3s','0022')",m.MAIL_DAEMON_OS_CAPTURE)

if __name__=='__main__':unittest.main()
