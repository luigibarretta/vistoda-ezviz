# Security policy

Report vulnerabilities privately to the repository owner; do not open a public
issue containing credentials, signed URLs, camera serials, packet captures or
video. The latest tagged release receives security fixes. Development commits
are not release artifacts.

Production rules:

- never expose the bridge directly to the Internet;
- keep the API token outside Git in a mode-0600 file; the EZVIZ session is
  stored sealed (`token.sealed` plus its key `store.key`, both mode 0600). A
  backup that contains both files still contains the session: protect backups
  of the data directory like the session itself;
- never pass account passwords or MFA codes on command lines;
- restrict network access to Home Assistant, SceneTrove and monitoring;
- retain only sanitized metrics and logs; do not retain canary media;
- rotate the bridge API token after any suspected disclosure and re-enroll the
  EZVIZ session after a session-token disclosure.

The bridge does not claim to make the proprietary EZVIZ protocol secure. It
reduces credential distribution and consumer coupling around the verified
vendor library.
