# Release Process

This document explains how releases are created for the OSH Tools project and how to troubleshoot common issues.

## Automated Releases via GitHub Actions

The project uses GitHub Actions to automatically create releases when a new tag is pushed. The workflow is defined in `.github/workflows/release-linux.yml`.

### How to Create a Release

1. Create and push a tag with the format `v*` (e.g., `v0.1.0`, `v1.2.3`):
   ```bash
   git tag v0.1.0
   git push origin v0.1.0
   ```

2. The GitHub Actions workflow will automatically:
   - Build the Linux binaries (`osh-relay`, `osh`, `osh-admin`)
   - Package them into `osh-tools-linux-x86_64.tar.gz`
   - Create a GitHub release with the tag
   - Upload the packaged artifacts to the release

### Required Repository Configuration

**IMPORTANT**: For the release workflow to work, you must configure the repository-level workflow permissions:

1. Go to your repository on GitHub
2. Navigate to **Settings** > **Actions** > **General**
3. Scroll down to **Workflow permissions**
4. Select **"Read and write permissions"** (NOT the default "Read repository contents and packages permissions")
5. Ensure **"Allow GitHub Actions to create and approve pull requests"** is checked (if needed)
6. Click **Save**

Without this configuration, the workflow will fail with a 403 error even if the workflow file has `permissions: contents: write` declared.

## Troubleshooting

### 403 Error: "Resource not accessible by personal access token"

**Error message:**
```
⚠️ GitHub release failed with status: 403
{"message":"Resource not accessible by personal access token","documentation_url":"https://docs.github.com/rest/releases/releases#create-a-release","status":"403"}
```

**Common causes and solutions:**

1. **Repository-level permissions are read-only (most common)**
   - **Solution**: Follow the "Required Repository Configuration" steps above to enable "Read and write permissions"
   - Even if you set `permissions: contents: write` in the workflow file, the repository-level setting overrides it

2. **The GITHUB_TOKEN is not being passed to the action**
   - **Solution**: The workflow now explicitly passes `token: ${{ secrets.GITHUB_TOKEN }}` to the `softprops/action-gh-release` action
   - This is already configured in the current workflow file

3. **Tag protection rules**
   - **Solution**: Check if you have tag protection rules that prevent the workflow from accessing certain tags
   - Go to **Settings** > **Tags** and review any protection rules

4. **Using a Personal Access Token (PAT) with insufficient permissions**
   - If you're using a custom PAT instead of `GITHUB_TOKEN`, ensure it has:
     - `repo` scope (full control of private repositories)
     - OR at minimum: `public_repo` scope (for public repositories)
   - However, for releases in the same repository, the default `GITHUB_TOKEN` is sufficient and recommended

### Verifying the Fix

After configuring the repository permissions:

1. Delete the failed release (if one was created): Go to **Releases** > click the failed release > **Delete**
2. Delete the tag locally and remotely:
   ```bash
   git tag -d v0.1.0
   git push origin :refs/tags/v0.1.0
   ```
3. Recreate and push the tag:
   ```bash
   git tag v0.1.0
   git push origin v0.1.0
   ```
4. Monitor the workflow run in the **Actions** tab

## Manual Release (Fallback)

If the automated workflow continues to fail, you can create a release manually:

1. Build the binaries locally:
   ```bash
   cargo build --release -p osh-relay -p osh-cli
   ```

2. Package the artifacts:
   ```bash
   mkdir -p dist
   cp target/release/osh-relay dist/
   cp target/release/osh dist/
   cp target/release/osh-admin dist/
   tar -czf osh-tools-linux-x86_64.tar.gz -C dist .
   ```

3. Create a release on GitHub:
   - Go to **Releases** > **Draft a new release**
   - Choose your tag or create a new one
   - Upload `osh-tools-linux-x86_64.tar.gz`
   - Publish the release

## References

- [GitHub Actions Permissions](https://docs.github.com/en/actions/security-guides/automatic-token-authentication#permissions-for-the-github_token)
- [softprops/action-gh-release Documentation](https://github.com/softprops/action-gh-release)
- [GitHub REST API - Releases](https://docs.github.com/en/rest/releases/releases)
