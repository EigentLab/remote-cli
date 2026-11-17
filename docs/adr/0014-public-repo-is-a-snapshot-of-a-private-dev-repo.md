# The public repo is a snapshot of a private dev repo

Development happens in a private repo that also holds working notes: specs, tickets, research and agent instructions. The public repo `EigentLab/remote-cli` receives the code as snapshots: the dev repo's tree without the files marked `export-ignore` in `.gitattributes`, pushed to a branch, merged through a pull request and tagged there. Its history therefore starts at 1.0.0 and has one commit per update, not the dev repo's commits. We chose this over one public repo because the working notes are not meant to be published, and over filtering the dev history commit by commit because that is easy to get wrong and would publish internal commit messages.

## Consequences

- A release tag exists in both repos under the same name, on different commits: the snapshot's merge commit in the public repo, and the commit it was taken from in the dev repo.
- A pull request from outside is not merged in the public repo. Its patch is applied in the dev repo, with credit to its author, and reaches the public repo in the next snapshot.
- Releases before 1.0.0 exist only in the dev repo.
- A file that must stay private has to be listed in `.gitattributes` before the next snapshot; nothing else keeps it out.
