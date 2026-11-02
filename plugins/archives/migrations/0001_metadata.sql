CREATE TABLE archives (
    id uuid PRIMARY KEY,
    request_hash text NOT NULL,
    body text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
