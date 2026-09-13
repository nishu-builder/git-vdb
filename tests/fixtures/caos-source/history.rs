// Immutable source snapshots preserve historical results.
// A write produces a new content-addressed root; readers keep the old root.
// Deleting a document only changes the newly assembled snapshot.
pub fn retained_root(root: &str) -> String {
    root.to_owned()
}
