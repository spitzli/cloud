CREATE OR REPLACE FUNCTION webdock_rls.billing_site(customer text,project text,binding text,management_url text) RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE result jsonb;
BEGIN
 IF NOT webdock_rls.billing_admin(customer) THEN RAISE EXCEPTION 'Customer billing administrator required' USING ERRCODE='42501';END IF;
 SELECT jsonb_build_object('organizationID',b.organization_id,'bindingID',b.id,'projectID',p.id,'name',p.name) INTO result
 FROM webdock_auth.app_binding b JOIN webdock_auth."oauthClient" client ON client."clientId"=b.client_id AND NOT coalesce(client.disabled,false)
 JOIN webdock_auth.tenant_customer t ON t.organization_id=b.organization_id JOIN webdock_admin.projects p ON p.customer_id=t.customer_id JOIN webdock_admin.cms_instances i ON i.project_id=p.id
 WHERE t.customer_id=customer AND p.id=project AND b.id=binding AND b.enabled AND p.status='active' AND i.status='active' AND i.admin_u_r_l=management_url
 AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion WHERE project_id=p.id);
 RETURN result;
END $$;
