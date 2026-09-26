#!/bin/sh
# Set up a checkout for the StGit overlay workflow (see OVERLAY.md).
# Run from a clone of the fork made with `git clone -b overlay ...`.
# Safe to re-run; it only adds what is missing.
set -eu

UPSTREAM_URL=https://github.com/gingko/client.git
cd "$(git rev-parse --show-toplevel)"

git remote get-url upstream >/dev/null 2>&1 || git remote add upstream "$UPSTREAM_URL"

# Mirror the fork's StGit stack refs into their own namespace. Never fetch
# them straight into refs/stacks/*: that would overwrite local stack state on
# every fetch. overlay-adopt.sh copies a stack over deliberately.
spec='+refs/stacks/*:refs/remote-stacks/origin/*'
git config --get-all remote.origin.fetch | grep -qxF "$spec" ||
    git config --add remote.origin.fetch "$spec"

git fetch origin
git fetch upstream

# master mirrors upstream and is never committed to.
if ! git rev-parse -q --verify refs/heads/master >/dev/null; then
    git branch --track master upstream/master
else
    git branch --set-upstream-to=upstream/master master >/dev/null
    if [ "$(git symbolic-ref -q --short HEAD)" != master ] &&
        git merge-base --is-ancestor master upstream/master; then
        git branch -f master upstream/master
    fi
fi

git config alias.overlay-adopt '!sh tools/overlay-adopt.sh'
git config alias.overlay-publish '!sh tools/overlay-publish.sh'

if ! git rev-parse -q --verify refs/stacks/overlay >/dev/null; then
    sh tools/overlay-adopt.sh overlay
fi
stg series --description
