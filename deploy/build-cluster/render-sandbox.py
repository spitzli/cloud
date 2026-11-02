#!/usr/bin/env python3
"""Offline operator rendering; missing image pin denies actual customer runners."""
import argparse
from pathlib import Path
import re
import yaml

def render(image):
    if image is not None and not re.fullmatch(r'[a-z0-9][a-z0-9.:/_-]*@sha256:[a-f0-9]{64}',image):
        raise ValueError('An immutable runner image is required')
    documents=[]
    for original in yaml.safe_load_all((Path(__file__).parent/'foundation.yaml').read_text()):
        if not original or original['kind'].startswith('ValidatingAdmission'):continue
        item=yaml.safe_load(yaml.safe_dump(original).replace('webdock-builds','webdock-build-qualification'))
        if item['kind']=='Namespace':
            item['metadata']['labels']['pod-security.kubernetes.io/enforce']='baseline'
            item['metadata']['labels']['webdock.dev/profile']='rootless-idmap-sandbox'
            item['metadata']['annotations']={'webdock.dev/runner-image':image or ''}
        if item['kind']=='Role':
            item['rules'].append({'apiGroups':[''],'resources':['secrets'],'verbs':['create','get','delete']})
        if item['kind']=='ResourceQuota':item['spec']['hard']['secrets']='4'
        documents.append(item)
    return documents
if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--runner-image');args=parser.parse_args()
    try: print(yaml.safe_dump_all(render(args.runner_image),sort_keys=False))
    except ValueError as error:parser.error(str(error))
