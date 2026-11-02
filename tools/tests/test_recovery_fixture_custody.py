import unittest
from recovery_fixture_custody import (
    ComponentImage, PauseImage, component_delete, pause_delete, RUN_LABEL, KIND_LABEL, KIND,
)

class ImageCustodyTests(unittest.TestCase):
    def setUp(self):
        self.receipt=ComponentImage('123456abcdef','sha256:'+'a'*64)
        self.current={'Id':self.receipt.image_id,'RepoTags':[self.receipt.reference],
            'RepoDigests':[], 'Config':{'Labels':{RUN_LABEL:self.receipt.run,KIND_LABEL:KIND}}}
        self.protected={'sha256:'+'b'*64}

    def no_delete(self, receipt, current):
        emitted=[]
        try:emitted.append(component_delete(receipt,current,self.protected))
        except ValueError:pass
        self.assertEqual(emitted,[], 'guard must fail before any delete command can be emitted')

    def test_foreign_cached_ntfy_tag_cannot_emit_delete(self):
        self.current['RepoTags']=['binwiederhier/ntfy:latest']
        self.no_delete(self.receipt,self.current)

    def test_wrong_id_or_run_label_cannot_emit_delete(self):
        self.current['Id']='sha256:'+'c'*64
        self.no_delete(self.receipt,self.current)
        self.current['Id']=self.receipt.image_id
        self.current['Config']['Labels'][RUN_LABEL]='foreign-run'
        self.no_delete(self.receipt,self.current)

    def test_missing_label_wrong_kind_and_multiple_tags_cannot_emit_delete(self):
        for labels in [{},{RUN_LABEL:self.receipt.run,KIND_LABEL:'foreign-image'}]:
            current={**self.current,'Config':{'Labels':labels}}
            self.no_delete(self.receipt,current)
        current={**self.current,'RepoTags':[self.receipt.reference,'foreign/alias:retained']}
        self.no_delete(self.receipt,current)

    def test_protected_identity_and_extra_alias_are_never_removed(self):
        protected=ComponentImage(self.receipt.run,next(iter(self.protected)))
        self.current['Id']=protected.image_id
        self.no_delete(protected,self.current)
        self.current['Id']=self.receipt.image_id
        self.current['RepoTags'].append('someone/retained:latest')
        self.no_delete(self.receipt,self.current)

    def test_exact_labeled_component_returns_only_its_immutable_reference(self):
        self.assertEqual(component_delete(self.receipt,self.current,self.protected),
            ('image','rm',self.receipt.reference))

    def pause(self):
        digest='sha256:'+'d'*64
        return PauseImage('rancher/mirrored-pause@'+digest,digest,'sha256:'+'e'*64,True,
            'rancher/k3s@sha256:057cfd5a9d51b89f1f0ee139c28d5d5fdd5aa7ddeac2742a0ba90c6dbc0329bb','rancher/mirrored-pause:3.10.2')

    def test_docker29_canonical_pause_reference_in_repo_tags_is_exact_custody(self):
        p=self.pause()
        current={'Id':p.image_id,'RepoTags':[p.reference],'RepoDigests':[p.reference]}
        self.assertEqual(pause_delete(p,current,p.configuration_id,self.protected),('image','rm',p.reference))

    def test_pause_requires_absence_identity_config_and_no_other_aliases(self):
        p=self.pause()
        current={'Id':p.image_id,'RepoTags':[p.reference],'RepoDigests':[p.reference]}
        for changed,config,receipt in [
            ({**current,'Id':'sha256:'+'f'*64},p.configuration_id,p),
            ({**current,'RepoTags':['rancher/mirrored-pause:other']},p.configuration_id,p),
            (current,'sha256:'+'f'*64,p),
            (current,p.configuration_id,PauseImage(p.reference,p.image_id,p.configuration_id,False,p.source_k3s,p.required_version)),
        ]:
            emitted=[]
            try:emitted.append(pause_delete(receipt,changed,config,self.protected))
            except ValueError:pass
            self.assertEqual(emitted,[])

if __name__=='__main__':unittest.main()
