# ci-mirrors tooling

This repository contains the tooling to manage the contents of
[ci-mirrors.rust-lang.org](https://ci-mirrors.rust-lang.org).

The contents of this repository are licensed under either the MIT or the Apache
2.0 license, at your option.

> [!WARNING]
>
> ci-mirrors is designed to be used by repositories managed by the Rust project
> only. We provide no guarantees for third parties.

## Uploading new files

To upload a new file to ci-mirrors, open a new PR adding a new entry to one of
the TOML files in the `files/` directory. Each entry has the following schema:

* **`name`**: the name the file will have on ci-mirrors. It's possible to use
  slashes to define an hierarchy, for example prefixing the name of the file
  with the repository that uses it.

* **`source`**: the URL of the original file to mirror. The tooling will
  download the file from there automatically, so make sure no user interaction
  nor JavaScript is required to start the download. Redirects are followed.

* **`sha256`**: the SHA256 of the file to mirror. The upload will fail if the
  mirrored file doesn't match the hash.

* **`license`**: the licensing of the file. This is a *freeform* field: for
  artifacts built from open source code you should put the license identifier,
  for everything else you should put a link to the licensing terms.

You can add a new entry either by manually modifying a TOML file in the `files` directory,
or by using the following command:

```bash
$ cargo run -- add-file <source-url> --path <cdn-name> --toml-files <path-to-toml-file> [--license <license>]
```

Once the PR is merged, the file will be available at:

```
https://ci-mirrors.rust-lang.org/${name}
```

> [!NOTE]
>
> Some files are marked with `legacy = true`, and don't have a source nor a
> license. Those files were added to ci-mirrors before this tool was introduced.
> No new files should be marked with it.

## Modifying or deleting an uploaded file

It is not currently supported to modify or delete an uploaded file. Doing so
would break the repositories currently relying on that file. If you *really*
need to do so, please ask the infra team on Zulip.

> [!NOTE]
>
> Storage space in ci-mirrors is not a concern. If you need to upload a new
> version of a file, add it separately without deleting the old one.
