use super::*;

impl HostService {
    pub async fn request_initial_callback_recovery_with_authority(
        &self,
        actor: &Principal,
        authority: &StoredAuthority,
        plugin: &str,
        input: UpdateRequest,
    ) -> Result<InstallSnapshot> {
        let payload = json!({"kind":"update","initialCallbackRecovery":true,"request":input});
        self.request_release(
            actor,
            authority,
            plugin,
            payload,
            Some(input.version),
            None,
            input.review_sha256,
            input.expected_revision,
            input.approved_capabilities,
            input.idempotency_key,
            true,
        )
        .await
    }
}

pub(super) fn initial_callback_repair(retained: &Value, reviewed: &Value) -> bool {
    let Some(core) = retained.get("core") else {
        return false;
    };
    let (Some(origin), Some(domain)) = (core["origin"].as_str(), core["domain"].as_str()) else {
        return false;
    };
    let Ok(mut origin) = url::Url::parse(origin) else {
        return false;
    };
    if origin.scheme() != "https"
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
        || domain.parse::<std::net::IpAddr>().is_ok()
    {
        return false;
    }
    let Some(ip) = origin
        .host_str()
        .and_then(|s| s.parse::<std::net::Ipv4Addr>().ok())
        .filter(std::net::Ipv4Addr::is_private)
    else {
        return false;
    };
    if retained
        .get("hostAliases")
        .is_some_and(|v| v.as_array().is_none_or(|a| !a.is_empty()))
    {
        return false;
    }
    let Some(port) = origin.port_or_known_default() else {
        return false;
    };
    if !retained["egress"].as_array().is_some_and(|a| {
        a.iter()
            .any(|v| v["cidr"] == format!("{ip}/32") && v["port"] == port)
    }) {
        return false;
    }
    if origin.set_host(Some(domain)).is_err() || origin.host_str() != Some(domain) {
        return false;
    }
    let mut expected = retained.clone();
    expected["core"]["origin"] = json!(origin.as_str());
    expected["hostAliases"] = json!([{"ip":ip.to_string(),"hostnames":[domain]}]);
    expected == *reviewed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policies() -> (Value, Value) {
        let retained = json!({"core":{"origin":"https://10.0.0.4:3132", "domain":"core.internal",
            "serverCertificateSha256":"a".repeat(64),"certificate":"/private/core.crt",
            "privateKey":"/private/core.key","authority":"/private/ca.crt"},
            "egress":[{"cidr":"10.0.0.4/32","port":3132}],"hostAliases":[],
            "runtimeDomain":"plugins.internal","resources":{"cpuMillicores":500,"memoryBytes":268435456}});
        let mut reviewed = retained.clone();
        reviewed["core"]["origin"] = json!("https://core.internal:3132/");
        reviewed["hostAliases"] = json!([{"ip":"10.0.0.4","hostnames":["core.internal"]}]);
        (retained, reviewed)
    }

    #[test]
    fn initial_callback_recovery_preserves_the_exact_endpoint_and_tls_authority() {
        let (retained, reviewed) = policies();
        assert!(initial_callback_repair(&retained, &reviewed));
        assert!(!initial_callback_repair(&retained, &retained));
    }

    #[test]
    fn initial_callback_recovery_refuses_every_other_policy_change() {
        let (retained, reviewed) = policies();
        for (field, value) in [
            ("serverCertificateSha256", json!("b".repeat(64))),
            ("domain", json!("other.internal")),
            ("authority", json!("/private/foreign-ca.crt")),
        ] {
            let mut changed = reviewed.clone();
            changed["core"][field] = value;
            assert!(!initial_callback_repair(&retained, &changed));
        }
        for changed in [
            {
                let mut v = reviewed.clone();
                v["resources"]["cpuMillicores"] = json!(1000);
                v
            },
            {
                let mut v = reviewed.clone();
                v["egress"] = json!([{"cidr":"0.0.0.0/0","port":3132}]);
                v
            },
            {
                let mut v = reviewed.clone();
                v["hostAliases"][0]["ip"] = json!("10.0.0.5");
                v
            },
            {
                let mut v = reviewed.clone();
                v["core"]["origin"] = json!("http://core.internal:3132/");
                v
            },
        ] {
            assert!(!initial_callback_repair(&retained, &changed));
        }
    }
}
