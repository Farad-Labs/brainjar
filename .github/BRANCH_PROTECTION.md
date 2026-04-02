# Branch Protection Setup

The `farad-bots` account lacks admin access to set branch protection rules via API.
Configure these manually in GitHub → Settings → Branches → Add rule for `main`:

## main branch rules

- **Require a pull request before merging**: ✅
  - Required approving reviews: 1
  - Dismiss stale pull request approvals when new commits are pushed: optional
- **Require status checks to pass before merging**: ✅
  - Require branches to be up to date before merging: ✅
  - Required status checks:
    - `CI (ubuntu-latest)`
    - `CI (macos-latest)`
    - `CI (windows-latest)`
- **Do not allow bypassing the above settings**: optional
- **Allow force pushes**: ❌
- **Allow deletions**: ❌

## Notes

- The `dev` branch has been created from `main` via the API.
- PRs from `dev` → `main` will trigger the CI checks above.
