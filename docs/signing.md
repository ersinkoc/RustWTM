# Windows signing setup

Checked on 2026-11-04. Provider eligibility, fees and stock can change. Releases
0.25.0 and 0.26.0 are unsigned. Configuring this workflow does not change existing
release assets or complete identity validation; an approved signing account is
required for signed releases.

## Estonia: company and individual

An Estonian company is within Azure Artifact Signing's EU organization scope.
Individuals located in Estonia are outside its current Public Trust individual
scope, which is limited to the US and Canada. Use the exact validated legal
publisher name, not an arbitrary product name.
[Microsoft eligibility and setup](https://learn.microsoft.com/en-us/azure/artifact-signing/quickstart).

Company age needs confirmation before spending: the current Artifact Signing
quickstart does not state a minimum, but Microsoft's MSIX signing overview
still lists three years of verifiable tax history. For a newer company, ask
Microsoft which rule applies to its Organization/Public validation before
provisioning paid resources. Country eligibility alone does not guarantee
acceptance.
[Microsoft's MSIX eligibility statement](https://learn.microsoft.com/en-us/windows/msix/package/signing-package-overview).

For the company route, Microsoft's published Basic reference price is
USD 9.99/month for 5,000 signatures; regional pricing, tax and overage apply.
Confirm the actual offer in your Azure account before provisioning.
[Microsoft product/pricing information](https://azure.microsoft.com/en-us/products/artifact-signing).

For a personal publisher name, SSL.com offers Individual Validation certificates
and an optional eSigner cloud service. Confirm Estonia eligibility, identity
documents, certificate cost and the separate automation/signing plan with the
provider before buying. This workflow does not yet implement eSigner.
[SSL.com certificate options](https://www.ssl.com/products/software-integrity/code-signing/),
[identity validation](https://www.ssl.com/how-to/validation-process-for-document-signing-code-signing-and-ev-code-signing-certificates/).

SignPath Foundation offers free signing for accepted open-source projects, but
the certificate publisher is the foundation. Acceptance and the project's
technical/review obligations require a separate application. That integration
is not enabled by the Azure switch below.
[Foundation conditions](https://signpath.org/terms).

Certum also lists a personal open-source cloud certificate, prefixed with
"Open Source Developer". Its product page showed out of stock during this
review; do not assume it can currently be purchased.
[Certum product](https://shop.certum.eu/open-source-code-signing-on-simplysign.html).

Signing establishes publisher identity and file integrity. SmartScreen reputation
can still need time to develop; neither ordinary nor EV signing guarantees an
immediate absence of SmartScreen warnings.
[Microsoft SmartScreen guidance](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation).

## Company: owner-operated Azure setup

For a company younger than three years, resolve the eligibility question above
with Microsoft before step 1.

1. Use a paid Azure subscription associated with the company's Entra tenant.
2. Register `Microsoft.CodeSigning`, create an Artifact Signing account, submit
   Organization/Public identity validation and finish the requested checks in
   the Azure portal. Provide the company's legal details and supporting material
   directly to Microsoft, not in repository files or chat.
3. After validation completes, create an active **Public Trust** certificate
   profile. Copy the account's endpoint, account name, profile name and exact
   publisher display name.
4. Create an Entra application/service principal for this repository. Add a
   GitHub OIDC federated credential with issuer
   `https://token.actions.githubusercontent.com`, audience
   `api://AzureADTokenExchange` and subject
   `repo:ersinkoc/RustWTM:environment:windows-signing`.
5. Assign that principal **Artifact Signing Certificate Profile Signer** at
   the specific certificate-profile scope. Do not use a client secret or export
   a certificate/private key for this workflow.

[Setup](https://learn.microsoft.com/en-us/azure/artifact-signing/quickstart),
[signer role and profile scope](https://learn.microsoft.com/en-us/azure/artifact-signing/tutorial-assign-roles),
[signing integration](https://learn.microsoft.com/en-us/azure/artifact-signing/how-to-signing-integrations).

## Repository configuration

Create the GitHub environment `windows-signing` for the OIDC subject above.
Set the following **repository-level Actions variables**. These are identifiers
and public metadata, not private keys or passwords.

| Variable | Value |
| --- | --- |
| `AZURE_CLIENT_ID` | Signing application's client ID |
| `AZURE_TENANT_ID` | Company's Entra tenant ID |
| `AZURE_SUBSCRIPTION_ID` | Subscription containing the signing account |
| `WINDOWS_SIGNING_ENDPOINT` | Endpoint copied from the signing account |
| `WINDOWS_SIGNING_ACCOUNT` | Artifact Signing account name |
| `WINDOWS_SIGNING_PROFILE` | Active Public Trust certificate profile |
| `WINDOWS_SIGNING_PUBLISHER` | Exact certificate publisher simple name |
| `WINDOWS_SIGNING_PROVIDER` | Set **last**, to `azure-artifact-signing` |

An unset provider or `none` keeps the unsigned path. Other values fail release
preflight. When Azure is selected, missing configuration, signing failures,
untrusted signatures, missing timestamps or a different publisher block
publication; there is no fallback to the unsigned artifact.

## Artifact sequence

The read-only build job compiles and tests the project and uploads an unsigned
intermediate artifact. A separate Windows signing job uses OIDC and signs only
the four project executables: GUI, CLI, service and hardware provider. Upstream
native libraries are not relabeled as our own publisher.

The installer is rebuilt from that signed executable payload, then the MSI is
signed separately. Both phases use SHA-256 and timestamping. Windows validates
trust, exact publisher and timestamp before packaging proceeds. ZIP and MSI
checksums are generated from the **final signed bytes**.

The provenance job attests the final ZIP/MSI; the publish job uploads only that
selected artifact. Provenance identifies the producing GitHub workflow and
source; it is separate from Windows Authenticode publisher trust.

To verify a future release after downloading it:

```powershell
scripts/ci/verify-signatures.ps1 -Path .\rustwtm.exe,.\wtm.exe,.\wtm-service.exe,.\hardware\wtm-hardware.exe -ExpectedPublisher 'EXACT VALIDATED COMPANY NAME' -RequireTimestamp
scripts/ci/verify-signatures.ps1 -Path .\RustWTM-VERSION-x64.msi -ExpectedPublisher 'EXACT VALIDATED COMPANY NAME' -RequireTimestamp
gh attestation verify .\RustWTM-VERSION-x64.msi --repo ersinkoc/RustWTM --signer-workflow ersinkoc/RustWTM/.github/workflows/release.yml --deny-self-hosted-runners
```

No real Azure signing can be certified until the owner completes validation,
configures the variables and a signed release passes downloaded-asset checks.
Existing unsigned update/install paths are not retroactively publisher-pinned
by adding this release workflow; enforcing publisher trust inside the updater
is a separate follow-up before describing updates as publisher-validated.
