"""Provider isolation contracts; these tests start no services and read no secrets."""
import unittest
from unittest.mock import patch
from pathlib import Path
from qualification import Fixture, NETWORK
from qualification_review import Review

class ProviderIsolation(unittest.TestCase):
 def fixture(self,state):
  fixture=Review.__new__(Review);fixture.s=state;return fixture
 def test_review_never_falls_back_to_existing_provider_or_network(self):
  fixture=self.fixture({})
  with self.assertRaises(KeyError):_ = fixture.network
  with self.assertRaises(KeyError):_ = fixture.minio_container
  with self.assertRaises(KeyError):fixture.minio_credentials()
 def test_owned_credentials_do_not_read_shared_private_configuration(self):
  fixture=self.fixture({'network':'owned-network','minioContainer':'owned-minio','ownedMinioUser':'owned-user','ownedMinioSecret':'ephemeral-fixture-secret'})
  with patch.object(Path,'read_text',side_effect=AssertionError('shared private configuration must not be read')):
   self.assertEqual(fixture.minio_credentials(),('owned-user','ephemeral-fixture-secret'))
  self.assertEqual(fixture.network,'owned-network');self.assertEqual(fixture.minio_container,'owned-minio')
 def test_legacy_harness_default_remains_explicitly_compatible(self):
  fixture=Fixture.__new__(Fixture);fixture.s={}
  self.assertEqual(fixture.network,NETWORK);self.assertEqual(fixture.minio_container,'webdock-dev-minio-1')

if __name__=='__main__':unittest.main()
