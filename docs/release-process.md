# A new release :rocket:

1. Prepare the various text files with a bump to a new version.
You can utilise the helper script in `scripts/prepare_release.sh <DESIRED_VERSION>` for this purpose.
E.g. `scripts/prepare_release.sh 0.0.20`
1. Create a pull request with the newly prepared files.
1. Ensure you have the latest changes available after a merge to `main` from the changes above.
1. Create your desired tag with `git tag` or equivalent.
**Reminder:** `kubit` uses [semver](https://semver.org/). E.g. `git tag v0.0.20`
1. Push the newly created tag using `git push origin <tag>` or equivalent.
1. CI will create a [draft release](https://github.com/kubecfg/kubit/releases) for the new tag. Curate some notes and publish the release!
