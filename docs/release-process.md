# A new release :rocket:

1. Prepare the various text files with a bump to a new version, an example is shown [here](https://github.com/kubecfg/kubit/pull/479).
1. Ensure you have the latest changes available after a merge to `main`.
1. Create your desired tag with `git tag` or equivalent.
  a. **Reminder:** `kubit` uses [semver](https://semver.org/).
1. Push the newly created tag using `git push origin <tag>` or equivalent.
1. CI will create a [draft release](https://github.com/kubecfg/kubit/releases) for the new tag. Now curate some notes and publish the release!
