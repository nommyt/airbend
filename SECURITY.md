# Security policy

Airbend's privileged daemon controls cooling hardware. Security-sensitive areas include the
Unix socket, configuration validation, hardware writes, recovery journals, and any shell or
command execution in the Omarchy integration.

Report vulnerabilities privately to me@nommyt.com. When GitHub private vulnerability reporting
is enabled, the repository's Security tab can also be used. Do not disclose an unpatched
vulnerability in a public issue. Include the affected version, impact, reproduction steps, and
relevant logs with credentials and personal identifiers removed.

This project is maintained by one person. There is no guaranteed response time or bug bounty.
Confirmed issues will be investigated and fixes coordinated with the reporter.

The latest `main` branch is the supported development line. Airbend is pre-1.0; no LTS branches
or support commitments are provided. Hardware-driver and firmware vulnerabilities should also
be reported to their respective maintainers.
