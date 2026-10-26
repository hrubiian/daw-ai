#!/usr/bin/env bash
# Commit non-ignored changes, then interactively confirm the configured push.
set -euo pipefail

if [[ ${1:-} == --help || ${1:-} == -h ]]; then
    echo "Usage: $0 \"Commit message\""
    echo "Stages all non-ignored changes, commits if needed, then asks before pushing."
    echo "Uses the current branch's upstream, or origin/current-branch if none exists."
    exit 0
fi
if [[ $# -ne 1 || -z ${1//[[:space:]]/} ]]; then
    echo "Usage: $0 \"Commit message\"" >&2
    exit 2
fi
cd "$(git rev-parse --show-toplevel)"
branch=$(git symbolic-ref --quiet --short HEAD) || {
    echo "Detached HEAD: switch to a branch first." >&2
    exit 1
}
remote=$(git config --get "branch.$branch.remote" || true)
merge_ref=$(git config --get "branch.$branch.merge" || true)
if [[ -z $remote && -z $merge_ref ]]; then
    remote=origin
    merge_ref="refs/heads/$branch"
    set_upstream=true
else
    set_upstream=false
fi
if [[ -z $remote || $remote == . || $merge_ref != refs/heads/* ]]; then
    echo "An external remote and branch upstream are required." >&2
    exit 1
fi
# Refuse ambiguous multi-destination pushes.
mapfile -t destinations < <(git remote get-url --push --all "$remote")
if [[ ${#destinations[@]} -ne 1 || -z ${destinations[0]} ]]; then
    echo "Configure exactly one push URL for $remote first." >&2
    exit 1
fi
if [[ -n $(git diff --name-only --diff-filter=U) ]]; then
    echo "Resolve merge conflicts before committing." >&2
    exit 1
fi

git status --short
git add -A
git diff --cached --check
if git diff --cached --quiet; then
    echo "No changes to commit. Existing commits can still be pushed."
else
    git commit -m "$1"
fi

printf '\nDestination: %s\nRemote branch: %s\n' "${destinations[0]}" "${merge_ref#refs/heads/}"
tracking_ref="refs/remotes/$remote/${merge_ref#refs/heads/}"
if git show-ref --verify --quiet "$tracking_ref"; then
    echo "Pending commits relative to the last fetched remote state:"
    git log --oneline "$tracking_ref..HEAD"
else
    echo "No local tracking reference; this may publish the branch's full history."
    git log -5 --oneline
fi
echo "Pushing uploads these commits and their file contents to the destination above."
if ! { printf 'Type PUSH to confirm (anything else cancels): ' > /dev/tty; } 2>/dev/null; then
    echo "Interactive terminal required. Commit kept locally; nothing pushed." >&2
    exit 1
fi
answer=''
IFS= read -r answer < /dev/tty || true
if [[ $answer != PUSH ]]; then
    echo "Push cancelled. Commit kept locally."
    exit 0
fi
if [[ $set_upstream == true ]]; then
    git push --set-upstream -- "$remote" "HEAD:$merge_ref"
else
    git push -- "$remote" "HEAD:$merge_ref"
fi
