-- Private binary tickets have no signed invocation envelope. Ordinary authority remains 16KiB.
-- Only the sealed storage object gets extra room; original authority/configuration fences stay.
ALTER TABLE webdock_auth.plugin_invocation DROP CONSTRAINT plugin_invocation_session_context_check;
ALTER TABLE webdock_auth.plugin_invocation ADD CONSTRAINT plugin_invocation_session_context_check CHECK (
 octet_length(session_context::text)<=65536*128+65536
 AND (
  octet_length((session_context #- '{nativeAdmission,configuration}')::text)<=16384
  OR coalesce((
   kind='action'
   AND jsonb_typeof(session_context->'storageTransfer')='object'
   AND (session_context->'storageTransfer') ?& ARRAY['sealed','expiresAt','state']
   AND ((session_context->'storageTransfer') - ARRAY['sealed','expiresAt','state'])='{}'::jsonb
   AND jsonb_typeof(session_context#>'{storageTransfer,sealed}')='string'
   AND octet_length(session_context#>>'{storageTransfer,sealed}')<=32768
   AND jsonb_typeof(session_context#>'{storageTransfer,expiresAt}')='number'
   AND session_context#>>'{storageTransfer,state}' IN ('pending','consuming','complete')
   AND octet_length(((session_context-'storageTransfer') #- '{nativeAdmission,configuration}')::text)<=16384
   AND octet_length((session_context #- '{nativeAdmission,configuration}')::text)<=65536
  ),false)
 )
);
