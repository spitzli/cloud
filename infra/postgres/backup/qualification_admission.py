"""Render supplemental native backup admission; applying it is an explicit operator step.

Also regenerate boundary.documents([...CNPG, PostgreSQL, PgBouncer, SIDECAR_PIN]) so
its existing image allowlist admits the approved sidecar. No running cluster is read.
"""
import json
import re

def documents(plugin,sidecar):
 if plugin==sidecar or any(not re.fullmatch(r'[^\s@]+@sha256:[a-f0-9]{64}',v) for v in [plugin,sidecar]):raise ValueError('Two distinct immutable provenance-approved pins required')
 bound="has(c.resources) && has(dyn(c.resources).requests) && has(dyn(c.resources).limits) && ['cpu','memory','ephemeral-storage'].all(r, r in dyn(c.resources).requests && r in dyn(c.resources).limits && quantity(dyn(c.resources).requests[r]).isGreaterThan(quantity('0')) && quantity(dyn(c.resources).limits[r]).isGreaterThan(quantity('0')))"
 # A restartable init container contributes for the lifetime of the PostgreSQL/recovery pod.
 expr="!has(object.spec.initContainers) || object.spec.initContainers.filter(c, c.name == 'plugin-barman-cloud').all(c, c.image == "+json.dumps(sidecar)+" && has(c.restartPolicy) && c.restartPolicy == 'Always' && "+bound+")"
 name='webdock-database-backup-pods'
 return [{'apiVersion':'admissionregistration.k8s.io/v1','kind':'ValidatingAdmissionPolicy','metadata':{'name':name,'labels':{'webdock.dev/database-boundary-version':'1'}},'spec':{'failurePolicy':'Fail','matchConstraints':{'namespaceSelector':{'matchLabels':{'webdock.dev/workload-class':'database'}},'resourceRules':[{'apiGroups':[''],'apiVersions':['v1'],'operations':['CREATE','UPDATE'],'resources':['pods'],'scope':'Namespaced'}]},'validations':[{'expression':expr,'message':'Backup restartable init sidecars require the approved immutable image and bounded CPU, RAM and ephemeral storage.'}]}},{'apiVersion':'admissionregistration.k8s.io/v1','kind':'ValidatingAdmissionPolicyBinding','metadata':{'name':name},'spec':{'policyName':name,'validationActions':['Deny']}}]
