-- Bootstrap may call only explicitly granted billing claim predicates; no billing-table access is added.
GRANT USAGE ON SCHEMA webdock_rls TO webdock_auth_bootstrap;
