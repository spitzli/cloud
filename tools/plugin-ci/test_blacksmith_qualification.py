import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest

SPEC=importlib.util.spec_from_file_location('qualification',Path(__file__).with_name('qualify_blacksmith.py'))
MODULE=importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class ProbeExport(unittest.TestCase):
    def test_export_digest_must_match_the_single_actual_manifest_blob(self):
        with tempfile.TemporaryDirectory() as td:
            root=Path(td);archive=root/'probe.oci.tar';metadata=root/'metadata.json'
            raw=b'{"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json"}'
            digest=hashlib.sha256(raw).hexdigest()
            def write(blob=raw,descriptors=None):
                index=json.dumps({'schemaVersion':2,'manifests':descriptors or [{'digest':'sha256:'+digest}]}).encode()
                with tarfile.open(archive,'w') as output:
                    for name,data in [('index.json',index),('blobs/sha256/'+digest,blob)]:
                        info=tarfile.TarInfo(name);info.size=len(data);output.addfile(info,io.BytesIO(data))
                metadata.write_text(json.dumps({'containerimage.digest':'sha256:'+digest}))
            write()
            self.assertEqual(MODULE.verify_oci_export(archive,metadata),'sha256:'+digest)
            write(blob=b'altered')
            with self.assertRaises(ValueError):MODULE.verify_oci_export(archive,metadata)
            write(descriptors=[{'digest':'sha256:'+digest},{'digest':'sha256:'+digest}])
            with self.assertRaises(ValueError):MODULE.verify_oci_export(archive,metadata)
            write();metadata.write_text(json.dumps({'containerimage.digest':'sha256:'+'0'*64}))
            with self.assertRaises(ValueError):MODULE.verify_oci_export(archive,metadata)


if __name__=='__main__':unittest.main()
