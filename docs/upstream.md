# Upstream relationship

Forge originated as a fork of
[ForgeAILab/forge](https://github.com/ForgeAILab/forge). This origin remains
part of the project's history and attribution. The project retains its
original MIT licensing obligations and does not remove upstream credit.

The architectural decision recorded in Plan PR0 is now explicit:

* this repository is independently maintained;
* architectural compatibility with ForgeAILab/forge is no longer a goal;
* downstream work does not wait for upstream review or release cadence;
* preserved infrastructure is kept because it is useful, not because it must
  remain structurally compatible;
* isolated upstream fixes may be selectively reviewed and cherry-picked when
  their behavior and security fit this architecture;
* domain-model, orchestration, cognition, and public-surface changes are
  designed here first and are not upstream synchronization work.

The preserved areas are primarily daemon/process integration, harness
adapters, Git, workspaces, configuration, local SQLite deployment, logs,
events, REST, MCP, CLI, and web infrastructure. The target architecture
replaces the upstream-derived cognition and role assumptions with Actors,
harness-bound Agents, multi-actor Roles, explicit Executions, HarnessSessions,
WorkUnits, durable collaboration, and event-driven orchestration.

## Remotes

The checkout currently uses:

~~~text
origin    https://github.com/AlRos14/forge.git
upstream  https://github.com/ForgeAILab/forge.git
~~~

Remote names are operational configuration, not authority. Before adopting an
upstream change, inspect the current commit, affected readers/writers,
migrations, public surfaces, and tests. Never merge it merely to reduce
divergence.

## Attribution and license

The project continues to credit ForgeAILab and its contributors. The MIT
license in the repository remains in force. A future product rename must
preserve this attribution and provide configuration/data discovery for existing
local users.

The migration contract is in
[migration/architecture-v2.md](migration/architecture-v2.md); the target
architecture is in [architecture.md](architecture.md).
