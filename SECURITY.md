# Security policy

Apelora Fabric is an early-stage project and has not been independently audited. Do not deploy on production hypervisor hosts until the control plane, host agent, network isolation, authentication, and privilege boundaries have undergone security review.

## Reporting a vulnerability

**Do not open a public issue** for security problems, and do not post exploit details or secrets publicly.

Report privately using GitHub's **private vulnerability reporting**: open the repository's **Security** tab and choose **Report a vulnerability** (https://github.com/Apelora/apelora-fabric/security/advisories/new).

Please include the affected component (Studio, Controller, Node, Catalog, Fabric), version or commit, reproduction steps, and the impact you observe.

## What to expect

- Acknowledgement within 5 business days
- An initial assessment and next steps within 14 days
- Coordinated disclosure: we will agree on a fix and disclosure timeline with you and credit you if you wish

## Scope

In scope: all code in this repository, including the control plane API, the deployment worker, the Incus adapter, the Studio and Fabric Console, and the catalog/assembly formats. Out of scope: vulnerabilities in Incus, LXC or QEMU themselves (report those to their upstream security teams), in third-party module images, or in upstream dependencies (report upstream; tell us too if we are affected).

## Supported versions

There are no stable releases yet. Security fixes are applied to `main`.
