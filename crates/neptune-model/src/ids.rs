use serde::{Deserialize, Serialize};

macro_rules! identity {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(u64);

        impl $name {
            pub const fn new(value: u64) -> Self {
                Self(value)
            }
            pub const fn get(self) -> u64 {
                self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

identity!(PaneId);
identity!(WorkspaceId);
identity!(WorkspaceGroupId);
identity!(SplitId);
identity!(SessionGeneration);
identity!(ProjectId);
identity!(WindowId);

impl WindowId {
    /// The original native window; retained even when its workspace list is empty.
    pub const ROOT: Self = Self::new(1);
}
