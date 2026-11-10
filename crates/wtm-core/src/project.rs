//! Canonical public project identity, shared by desktop links and update checks.
pub const HOMEPAGE: &str = "https://rustwtm.com";
pub const REPOSITORY: &str = "https://github.com/rustwtm/rustwtm";
pub const ORGANIZATION_URL: &str = "https://oxog.net";
pub const MAINTAINER: &str = "OXOGNET OÜ";
pub const LOCATION: &str = "Estonia";
pub const PUBLIC_REPOSITORY: &str = REPOSITORY;
pub const LATEST_RELEASE: &str = "https://github.com/rustwtm/rustwtm/releases/latest";
pub const LATEST_RELEASE_API: &str = "https://api.github.com/repos/rustwtm/rustwtm/releases/latest";

/// Repository identity verified before transfer. A copy/fork has a different ID.
pub const REPOSITORY_ID: u64 = 1_425_760_656;
/// Identity-based discovery follows the same repository across owner/name changes.
/// Never fall back to another repository simply because its name looks familiar.
pub const UPDATE_API: &str = "https://api.github.com/repositories/1425760656/releases/latest";
pub const UPDATE_PAGE: &str = LATEST_RELEASE;
