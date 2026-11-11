# Automatic website updates

The product website source lives in `RustWTM/rustwtm-site`. Its deployment workflow validates the official stable release, package/checksum pairs and changelog, then publishes only when needed.

## Release presentation package

`Release` calls the reusable `Website media` workflow before publication. That workflow checks out the exact release tag, renders every navigation page in English with both themes and window sizes, renders the device and quick-setup fixtures, and packages optimized WebP captures as `rustwtm-website-media.zip` with a SHA-256 sidecar. New navigation pages must have a render; missing captures stop publication rather than retaining an older version label.

The bounded manifest binds every capture to its release tag and source commit, with individual hashes and original dimensions. Images contain fixture data, not developer-host telemetry. The package is included in the release provenance job and is published alongside the installers. The website derives homepage highlights from the published release notes and imports the matching package as one validated content snapshot.

For a previously published stable version, run `Website media` manually with its tag. Before rendering, the backfill verifies any existing package against the real tag commit, both public asset digests, the sidecar and every image hash. A complete package is reused without changing its published bytes; an invalid or incomplete existing package stops the run. This matters because the About view includes measured frame timings, so a second real render can differ even at the same source commit. If no package exists, rendering and publication proceed normally. Completion starts the existing website notification workflow. It does not rebuild or replace installer binaries.

`.github/workflows/update-website.yml` requests a website content check after:

- Successful completion of the trusted `Release` publisher. This also covers releases published with `GITHUB_TOKEN`, whose release event cannot trigger another workflow.
- Publication or editing of a stable GitHub release.
- Changes to `CHANGELOG.md` on main.

The notifier requires the private RustWTM Website Sync app installed on **rustwtm-site only**, with Actions read/write and implicit Metadata read. Set `WEBSITE_SYNC_APP_CLIENT_ID` as a repository variable and `WEBSITE_SYNC_APP_PRIVATE_KEY` as an encrypted Actions secret in the product repository. No personal access token or site content-write permission is required.

The job does not check out product source or execute event-provided code. The pinned official token action requests access only to `rustwtm-site` with Actions write, and revokes the token when the job ends. It dispatches the site's `deploy.yml` on main with `check_only=true`. The receiver independently fetches the trusted product data; event payloads cannot replace its source URLs or branch.

Duplicate notifications are safe: the website skips publication when its verified snapshot matches the live receipt, and retries a previously committed but unpublished snapshot. Failed notification requests fail visibly after three attempts. The website's existing schedule remains a recovery fallback, not the primary update trigger.

To verify or retry notification without publishing a release:

```powershell
gh workflow run update-website.yml --repo RustWTM/RustWTM --ref main
```

Follow **Update product website** in the product repository and **Build and deploy GitHub Pages** in the website repository. A successful notification means the dispatch was accepted; publication is complete only when the website workflow and live `/updates.json` agree.
