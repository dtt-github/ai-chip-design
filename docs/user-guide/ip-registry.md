# IP registry

The **IP** panel publishes a project as a reusable block and imports blocks from other projects
(`wafer-api/src/ip.rs`). Imported blocks can then be placed on the [Blocks](blocks.md) canvas.

## Publishing

Choose a version (semver, higher than the last; versions never change once published) and a
description. Publishing takes the project's top module (`DESIGN_NAME` in `config.json`) and every
module it uses, with their files, at one commit; testbenches stay out. It requires:

- an `SPDX-License-Identifier` header in every published file (they combine into the block's
  licence, `A AND B`);
- the project's simulation gate passing at that commit.

The version records the commit, the ports, the parameters, the gate report and each file's sha256.
A block name belongs to the project that first published it. Blocks are visible to whoever can see
the source project.

## Importing

Pick a block and version; the panel shows the licence verdict, the obligations and exactly what
the import will change before you confirm. An import:

- copies the files, after checking them against their published hashes, to `ip/<block>/`;
- adds them to `VERILOG_FILES` in the project's `config*.json` files;
- pins the block, version, source commit, licence and file hashes in `ip.lock.json`;

all in one commit by the person importing. Importing another version replaces the old files and
pin. Module names that would clash with the project's are refused.

## Licence checks

Every project declares an SPDX licence (chosen when it's created, default Apache-2.0, written into
the starter files). A block's licence is judged against it (`wafer-api/src/licence.rs`):

- permissive licences are compatible (except Apache-2.0 into GPL-2.0-only);
- file-level copyleft (CERN-OHL-W, MPL, LGPL) is compatible with conditions;
- strong copyleft (GPL, AGPL, CERN-OHL-S) only goes into a project whose licence satisfies it;
- `OR` takes the best option, `AND` the worst.

Incompatible imports are refused. A licence the check doesn't know can only be imported by a
project owner who accepts it explicitly, which is recorded in the lock file. The obligations are
written into the import commit's message. This is an automated check over common licences, not
legal advice.
