CREATE TABLE resources(id uuid PRIMARY KEY, body text NOT NULL);
CREATE TABLE operations(
    id uuid PRIMARY KEY,
    resource uuid NOT NULL REFERENCES resources(id),
    fingerprint text NOT NULL,
    result text
);
