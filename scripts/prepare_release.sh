#!/bin/sh

latest_tag=$(git describe --tags --abbrev=0)
echo "Latest tag: ${latest_tag}"

# Removing the proceeding v as this is not used everywhere.
# E.g. Cargo.toml uses the plain version number.
latest_version=${latest_tag//v}
echo "Latest version: ${latest_version}"

desired_version=$1

if [ -z $1 ]; then
    echo -e "\ndesired_version is required\n"
    echo -e "For example: prepare_release.sh 0.0.19\n"
    exit 1
fi

echo "Desired version: ${desired_version}"

# The lock file is excluded so that we do not manipulate it directly.
# Allow `cargo` to do this instead for safety.
grep \
    -r \
    --exclude-dir={target,.git} \
    --exclude="Cargo.lock" \
    -l "${latest_version}" | \
    xargs sed -i "s/${latest_version}/${desired_version}/" \
    && cargo check
