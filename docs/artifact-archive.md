# Historical verification evidence

The generated `artifacts/` files were removed from the current source tree.
The complete tracked snapshot remains available at commit
[`994604ee9d27c907d32630287c7aaba43eb7302e`](https://github.com/zevem/neptune/tree/994604ee9d27c907d32630287c7aaba43eb7302e/artifacts).
Historical documentation links use that fixed commit so later builds cannot
replace the evidence they cite. The snapshot contains 962 files, including
captures, raw reports, logs and isolated test state from accepted and failed runs.
These records establish observations of their recorded builds, not acceptance
of the current build.

To export the tracked snapshot without restoring generated files into the
checkout, run from the repository root:

```sh
git archive --format=tar.gz \
  --output=../neptune-artifacts-994604ee9d27c907d32630287c7aaba43eb7302e.tar.gz \
  994604ee9d27c907d32630287c7aaba43eb7302e artifacts/
```

The original root-level `pty-bench.log` was ignored and never committed. It is
absent from the GitHub snapshot; the reported baseline in `performance.md`
remains a historical observation without a published raw log.

Removing the files from the current tree does not remove them from Git history
or reduce existing clone history. No history rewrite is part of this cleanup.
For new local output, PR attachments and CI uploads, see
[verification artifacts](development.md#verification-artifacts).
