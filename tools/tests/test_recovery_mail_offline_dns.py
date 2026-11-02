import asyncio,importlib.util,pathlib,struct,unittest
p=pathlib.Path(__file__).with_name('recovery-admission-component.py');spec=importlib.util.spec_from_file_location('dns_runner',p);r=importlib.util.module_from_spec(spec);spec.loader.exec_module(r)
n={'__name__':'offline_dns_tests'};exec(compile(r.MAIL_OFFLINE_DNS,'verified-offline-dns','exec'),n)
class OfflineDNSTests(unittest.TestCase):
 def query(self):return struct.pack('!6H',123,0x100,1,0,0,0)+b'\x06public\x04test\0'+struct.pack('!2H',1,1)
 def test_only_negative_bounded_responses_without_resolution(self):
  q=self.query();a=n['negative_reply'](q);self.assertEqual(a[:2],q[:2]);self.assertEqual(a[3]&15,3);self.assertEqual(a[12:],q[12:]);self.assertEqual(a[6:12],bytes(6))
  self.assertIsNone(n['negative_reply'](b'x'*513));self.assertIsNone(n['negative_reply'](b'x'));self.assertIsNone(n['negative_reply'](struct.pack('!6H',123,0x8000,1,0,0,0)))
 def test_malformed_or_compressed_questions_return_formerr(self):
  for body in [struct.pack('!6H',123,0x100,2,0,0,0),struct.pack('!6H',123,0x100,1,0,0,0)+b'\xc0\x0c'+bytes(4)]:
   a=n['negative_reply'](body);self.assertEqual(len(a),12);self.assertEqual(a[3]&15,1);self.assertEqual(a[4:],bytes(8))
 def test_actual_udp_protocol_has_no_forwarding_and_retains_only_counts(self):
  async def check():
   loop=asyncio.get_running_loop();state={'received':0,'answered':0,'rejected':0,'ioErrors':0};transport,_=await loop.create_datagram_endpoint(lambda:n['NegativeDNS'](state),local_addr=('127.0.0.1',0));import socket
   client=socket.socket(socket.AF_INET,socket.SOCK_DGRAM);client.setblocking(False)
   try:
    await loop.sock_sendto(client,self.query(),transport.get_extra_info('sockname'));a,_=await asyncio.wait_for(loop.sock_recvfrom(client,512),1);self.assertEqual(a[3]&15,3);self.assertEqual(state,{'received':1,'answered':1,'rejected':0,'ioErrors':0})
   finally:client.close();transport.close();await asyncio.sleep(0)
  asyncio.run(check())
if __name__=='__main__':unittest.main()
