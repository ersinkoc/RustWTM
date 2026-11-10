# Canonical addresses and repository transfer

- Website: **https://rustwtm.com**
- Repository: **https://github.com/rustwtm/rustwtm**
- Downloads: **https://github.com/rustwtm/rustwtm/releases/latest**
- Support: **https://github.com/rustwtm/rustwtm/issues**

These addresses are prepared in the current development source. This document
does not establish that the GitHub transfer, website hosting, DNS or TLS setup
has been completed. At preparation time on 2026-11-10, the destination repository
returned HTTP 404 to the authenticated repository lookup; the original public
repository remained reachable. The updater and bundled installer now discover
releases through the verified repository ID **1425760656**. The identity-based
latest/tag API was probed successfully before transfer, including an
unauthenticated native-client metadata check. It follows the existing repository
rather than trusting an owner/name that could be reused.

## Prepared changes

Shared Rust project constants supply the desktop website/repository links and
release-check URLs. `UPDATE_API` uses
`https://api.github.com/repositories/1425760656/releases/latest`; the installer
uses the same repository ID for latest and explicit tags. Release pages,
installer URLs and matching checksum URLs come from that response, so they follow
the new location without a code switch. Failed-update recovery retains the
last discovered release page. Public app/help/bootstrap links retain the current compatibility URL until transfer;
GitHub redirects that URL afterwards, while canonical metadata names the destination.
Metadata reads have a one-MiB response cap and
short timeouts; installer download/checksum/install protections remain intact.
Workspace metadata uses rustwtm.com as the homepage. README,
download/install examples, security reporting, changelog comparisons, MSI help
links and signing instructions use the new addresses. Upgrade CI downloads its
pinned baseline releases from `GITHUB_REPOSITORY`, so it follows the actual
repository before and after transfer.

Author credits, copyright, Windows service/registry identities, MSI UpgradeCode,
product name and application-data paths stay stable. This move does not change
installation ownership, erase settings or require an MSI identity migration.

## Bridge release and older clients

The 0.28.0 source includes this bridge. Prefer publishing it before the
transfer, so updated clients already use the stable ID. If transfer happens first,
verify the original API redirect immediately: versions already installed cannot
be retroactively changed and still depend on that route to find their next
release. Both before/after-transfer fixtures preserve installer/checksum pairs;
the transport redirect and checksum rejection controls remain in the test suite.

Use an actual GitHub transfer, retaining repository ID, tags and releases. A
new repository created by copying files or forking has a different ID and is not
automatically trusted as the update source. Keep the repository public so the
application's unauthenticated API checks continue to work. Discovery failure,
missing installers or bad checksums must remain errors; do not silently select
another repository or bypass verification to make an update appear available.

## Transfer sequence

1. Confirm permission to transfer the current repository and create repositories
   in the `rustwtm` organization. Use GitHub's repository transfer flow, including
   the target name `rustwtm`; do not create an empty destination repository first.
2. Transfer the existing repository, retaining history, issues, tags and releases.
   Confirm completion in GitHub and verify the repository ID is unchanged.
3. Update local clones only after destination access works:

   ```powershell
   git remote set-url origin https://github.com/rustwtm/rustwtm.git
   git remote -v
   git ls-remote origin refs/heads/main
   ```

4. Verify every published release's expected assets/checksum files and the latest
   release setting. Confirm that the canonical release API, README raw install
   URL, issues, badges and release links resolve. Keep existing tag identities.
5. Review organization Actions permissions, environments, variables, signing
   approvals, branch protections and any external integration policies.
   If Azure signing is enabled, its external OIDC subject must match
   `repo:rustwtm/rustwtm:environment:windows-signing`. Do not assume an external
   identity provider is updated by a GitHub transfer.
6. Retain pre-transfer artifact attestations and exact source/digest evidence.
   Check verification of old artifacts after transfer; historical signer/source
   identities must not be guessed or silently rewritten. Verify a new release's
   provenance against the new workflow identity and its exact commit.
7. No updater constant switch is required. After transfer, verify the stable-ID
   latest/tag endpoints return the same repository's releases with the new page
   and download URLs. Check the original API redirect for older clients. Run full
   exact-commit CI, including both pinned upgrade baselines and transfer-safe
   discovery controls, then publish/download/verify the next release.

   ```powershell
   gh api repositories/1425760656 --jq '{id,full_name}'
   gh api repositories/1425760656/releases/latest --jq '{tag_name,html_url}'
   cargo test --locked -p wtm-core identity_release_metadata_smoke -- --ignored --nocapture --test-threads=1
   ```

   The expected repository ID remains `1425760656`; the resolved owner/name and
   release URLs must now use `rustwtm/rustwtm`. The native probe only reads
   metadata and does not download or install an executable.

8. Configure rustwtm.com on the chosen hosting provider, verify ownership, DNS and
   HTTPS, and connect its download/support links to the canonical repository.
   Website deployment is separate from repository transfer.

GitHub redirects previous repository links after a transfer, but reusing the
previous owner/name can remove those redirects. Existing installed clients may
still use the old release API, so test that route after transfer and avoid
reusing the original repository location.

Reference: [GitHub repository transfer behavior and prerequisites](https://docs.github.com/en/repositories/creating-and-managing-repositories/transferring-a-repository).
