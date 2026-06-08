# SSH Interface

The `ssh.*` interface lane is for remote machine access and operator-level
execution.

## Scope

- connect to remote hosts
- run remote commands
- inspect remote state
- move files to and from hosts
- deployment hops and operational checks

## Future capabilities

- `ssh.connect_host`
- `ssh.run_remote_command`
- `ssh.sync_workspace`
- `ssh.collect_logs`

## Boundaries

- do not treat SSH as a generic API client
- do not place REST endpoint work here
- SSH is for machine access and remote execution, not for modeling service
  resource contracts
