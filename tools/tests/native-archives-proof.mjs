import assert from 'node:assert/strict';
export async function runtimeSnapshot(f,stage){
 const namespaces=(await f.kubeJSON(['get','namespaces','-l','webdock.dev/plugin=archives'])).items,result=[];
 for(const ns of namespaces){const name=ns.metadata.name;
  const deployments=(await f.kubeJSON(['-n',name,'get','deployments'])).items,pods=(await f.kubeJSON(['-n',name,'get','pods'])).items;
  assert.equal(deployments.length,1);const deployment=deployments[0];
  assert(deployment.spec.template.spec.securityContext.runAsUser===65532||deployment.spec.template.spec.containers[0].securityContext.runAsUser===65532);
  const container=deployment.spec.template.spec.containers[0];assert(container.securityContext.readOnlyRootFilesystem);
  result.push({namespace:name,namespaceUID:ns.metadata.uid,deploymentUID:deployment.metadata.uid,image:container.image,readOnlyRootFilesystem:true,pods:pods.map(p=>({uid:p.metadata.uid,name:p.metadata.name,ready:p.status.containerStatuses?.every(c=>c.ready)??false,imageIDs:p.status.containerStatuses?.map(c=>c.imageID)??[]}))});
 }
 assert(result.length>0&&result.some(r=>r.pods.some(p=>p.ready)));await f.write('runtime-'+stage+'.json',result);return result;
}
