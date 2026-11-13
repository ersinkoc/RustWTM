# Historical release provenance after transfer

The active repository is [rustwtm/rustwtm](https://github.com/rustwtm/rustwtm).

Release **0.28.0** was built before the repository transfer. Its original source
commit is `621b8e10f227a7ab5dbbb9ebec9df3b51b9221a4`. The tag, binaries and
checksums were retained through transfer; the signed workflow identity still
names the previous repository. Those cryptographic statements must not be edited
to match current branding.

The attestation API lookup under the new repository returned HTTP 404 during
post-transfer checks. The original signed Sigstore bundles, retained from the
successful pre-transfer verification, are included here:

- [ZIP bundle](release-provenance/0.28.0/zip.sigstore.json)
- [MSI bundle](release-provenance/0.28.0/msi.sigstore.json)

Both were verified again after transfer against the original signer/workflow,
exact source digest and artifact bytes. They contain public certificates and
signed evidence, not signing keys. Keeping a bundle verifies historical signing
identity; it does not claim the old attestation was issued to the new owner.

Download the existing artifacts into a local `downloaded` directory, then run
from the repository root:

```powershell
gh attestation verify downloaded/rustwtm-x86_64-pc-windows-msvc.zip --bundle docs/release-provenance/0.28.0/zip.sigstore.json --repo ersinkoc/RustWTM --signer-workflow ersinkoc/RustWTM/.github/workflows/release.yml --source-digest 621b8e10f227a7ab5dbbb9ebec9df3b51b9221a4 --deny-self-hosted-runners
gh attestation verify downloaded/RustWTM-0.28.0-x64.msi --bundle docs/release-provenance/0.28.0/msi.sigstore.json --repo ersinkoc/RustWTM --signer-workflow ersinkoc/RustWTM/.github/workflows/release.yml --source-digest 621b8e10f227a7ab5dbbb9ebec9df3b51b9221a4 --deny-self-hosted-runners
```

The old name in these commands is intentional: it verifies the historical signed
identity from the supplied bundle. Future releases must be checked against their
actual new repository workflow identity. The numeric repository ID used for
update discovery remains **1425760656**.
