-- Directories people sign in with over LDAP with TLS (ADR 0007, docs/design/sign-in-methods.md).
-- Apelora binds as the person with the password they typed and reads their entry and groups
-- with that same bind, so no directory password is stored here. Only ldaps:// or ldap:// with
-- StartTLS are accepted (checked by the API and here).
CREATE TABLE directories (
    id                  uuid PRIMARY KEY,
    -- What the sign-in form calls it.
    name                text NOT NULL UNIQUE CHECK (length(name) BETWEEN 1 AND 60),
    -- ldaps://host[:port], or ldap://host[:port] with start_tls; also the `users.issuer` of
    -- the people who sign in with it.
    url                 text NOT NULL UNIQUE CHECK (url LIKE 'ldaps://%' OR url LIKE 'ldap://%'),
    start_tls           boolean NOT NULL DEFAULT false,
    -- The certificate authority to trust (PEM); none: the system's trusted roots.
    ca_pem              text CHECK (ca_pem IS NULL OR length(ca_pem) <= 65536),
    -- How the person's bind name is built: a DN such as uid={username},ou=people,dc=example,dc=org,
    -- or a principal name such as {username}@example.org.
    bind_template       text NOT NULL CHECK (position('{username}' IN bind_template) > 0),
    -- Where and how to find the person's entry after the bind (needed for principal names).
    base_dn             text,
    user_filter         text,
    id_attribute        text NOT NULL DEFAULT 'entryUUID',
    username_attribute  text NOT NULL DEFAULT 'uid',
    email_attribute     text NOT NULL DEFAULT 'mail',
    name_attribute      text NOT NULL DEFAULT 'cn',
    -- Groups from an attribute of the entry (e.g. memberOf), from a search, or both.
    group_attribute     text,
    group_base_dn       text,
    group_filter        text,
    group_name_attribute text NOT NULL DEFAULT 'cn',
    enabled             boolean NOT NULL DEFAULT true,
    created_at          timestamptz NOT NULL DEFAULT now(),
    created_by          text NOT NULL,
    updated_at          timestamptz NOT NULL DEFAULT now(),
    updated_by          text NOT NULL,
    CHECK (url LIKE 'ldaps://%' OR start_tls)
);
