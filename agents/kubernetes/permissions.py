#!/usr/bin/env python3
"""Print scoped RBAC as JSON. Does not apply anything or create credentials."""
import argparse
import json
from byok import allowed_namespace

def permissions(namespaces):
 if not namespaces or len(namespaces)>100 or any(not allowed_namespace(n,namespaces) for n in namespaces):raise ValueError('Choose allowed application namespaces')
 def role(kind,name,rules,namespace=None):
  return {'apiVersion':'rbac.authorization.k8s.io/v1','kind':kind,'metadata':{'name':name,**({'namespace':namespace} if namespace else {})},'rules':rules}
 def binding(kind,name,rolekind,namespace=None):
  return {'apiVersion':'rbac.authorization.k8s.io/v1','kind':kind,'metadata':{'name':name,**({'namespace':namespace} if namespace else {})},'roleRef':{'apiGroup':'rbac.authorization.k8s.io','kind':rolekind,'name':name},'subjects':[{'kind':'ServiceAccount','name':'spitzli-agent','namespace':'spitzli-agent'}]}
 def rule(group,resources,verbs):return {'apiGroups':[group],'resources':resources,'verbs':verbs}
 items=[{'apiVersion':'v1','kind':'Namespace','metadata':{'name':'spitzli-agent'}},{'apiVersion':'v1','kind':'ServiceAccount','metadata':{'name':'spitzli-agent','namespace':'spitzli-agent'},'automountServiceAccountToken':False}]
 items+=[role('ClusterRole','webdock-inventory',[rule('',['nodes','namespaces'],['get','list']),rule('storage.k8s.io',['storageclasses'],['get','list'])]),binding('ClusterRoleBinding','webdock-inventory','ClusterRole')]
 for namespace in sorted(set(namespaces)):
  rules=[rule('',['pods','services','persistentvolumeclaims'],['get','list']),rule('',['pods/log'],['get']),rule('apps',['deployments','statefulsets','daemonsets'],['get','list','patch']),rule('apps',['deployments/scale','statefulsets/scale'],['get','patch']),rule('batch',['jobs'],['get','list']),rule('batch',['cronjobs'],['get','list','patch']),rule('autoscaling',['horizontalpodautoscalers'],['get','list']),rule('networking.k8s.io',['ingresses'],['get','list']),rule('metrics.k8s.io',['pods'],['get','list'])]
  items += [role('Role','webdock-applications',rules,namespace),binding('RoleBinding','webdock-applications','Role',namespace)]
 return {'apiVersion':'v1','kind':'List','items':items}
if __name__=='__main__':
 parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--namespace',action='append',required=True);args=parser.parse_args()
 print(json.dumps(permissions(args.namespace),indent=2))
