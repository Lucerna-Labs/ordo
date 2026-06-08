# REST API Interface

The `rest.*` interface lane is for explicit REST-style HTTP integrations.

## Scope

- endpoint descriptions
- route and method handling
- request body preparation
- response validation
- resource synchronization
- CRUD-style external service operations

## Future capabilities

- `rest.describe_endpoint`
- `rest.prepare_request`
- `rest.validate_response`
- `rest.sync_resource`

## Boundaries

- REST is a specific API shape, not the bucket for every external integration
- keep generic service auth and non-REST integrations under `api.*`
- keep remote shell and deployment work under `ssh.*`
