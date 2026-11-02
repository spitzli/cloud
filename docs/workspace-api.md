# Workspace API

Scripts and CI pipelines use a workspace through API tokens. Owners and admins create them in the
console under **API tokens**: a name, read only or read and change, and 30 days to a year. The token
(`wdk_…`) is shown once; Core keeps only its SHA-256.

A token acts as the member who made it, in that workspace only. If the member loses access, the
token does too. Requests go to the API origin, without cookies or an `Origin` header.

```sh
export WEBDOCK_TOKEN=wdk_…   # from the console
API=https://api.example.eu

# Projects of the workspace
curl -H "Authorization: Bearer $WEBDOCK_TOKEN" $API/v1/api/projects

# Products in a project, with their actions and resources
curl -H "Authorization: Bearer $WEBDOCK_TOKEN" $API/v1/api/projects/$PROJECT/plugins

# Run an action, e.g. deploy a new image to an app
curl -H "Authorization: Bearer $WEBDOCK_TOKEN" -H "Content-Type: application/json" \
  -d '{"plugin":"hosting","action":"apps.deploy","input":{"resource_id":"…","image":"registry.example.eu/shop@sha256:…","port":8080}}' \
  $API/v1/api/projects/$PROJECT/actions

# Plan, limits and usage
curl -H "Authorization: Bearer $WEBDOCK_TOKEN" $API/v1/api/usage
```

An action's input is the one its plugin's manifest describes. Errors come back as
`{"error":{"code","message"}}`; a plan limit or an unverified domain is a `409` with the reason.
A read-only token gets `403` for actions that change something.
