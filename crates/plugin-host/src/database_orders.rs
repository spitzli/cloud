pub(super) fn create_allowed(enabled: bool, plugin: &str, action: &str, replay: bool) -> bool {
    enabled
        || replay
        || plugin != "postgres"
        || !matches!(action, "databases.create" | "instances.create")
}

#[cfg(test)]
mod tests {
    use super::create_allowed;

    #[test]
    fn retirement_blocks_both_new_postgres_allocations_but_not_maintenance_or_replay() {
        for action in ["databases.create", "instances.create"] {
            assert!(!create_allowed(false, "postgres", action, false));
            assert!(create_allowed(true, "postgres", action, false));
            assert!(create_allowed(false, "postgres", action, true));
            assert!(create_allowed(false, "another-plugin", action, false));
        }
        for action in [
            "databases.view",
            "databases.credentials",
            "databases.rotate",
            "databases.delete",
            "instances.delete",
        ] {
            assert!(create_allowed(false, "postgres", action, false));
        }
    }
}
