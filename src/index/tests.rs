#[cfg(test)]
mod tests {
    use crate::index::build::{Reference, build_index};
    use crate::index::partition_scheme::{PartitionScheme, TreePredicate, compute_tree_key};
    use crate::index::{PartitionSet, SpecialistIndex};
    use crate::{PACKED_DIMS, SCALE};

    #[test]
    fn specialist_index_roundtrip_predicts_nearest_labels() {
        let index_path = std::env::temp_dir().join(format!(
            "rinha-rust-tree-index-{}-{}.idx",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let mut references = Vec::new();
        for offset in 0..8 {
            let mut vector = [0i16; PACKED_DIMS];
            vector[0] = offset;
            references.push(Reference { vector, label: 0 });
        }
        for offset in 0..8 {
            let mut vector = [SCALE; PACKED_DIMS];
            vector[0] = SCALE - offset;
            references.push(Reference { vector, label: 1 });
        }
        let index_bytes =
            build_index(references, 64, PartitionScheme::recommended()).expect("build index");
        std::fs::write(&index_path, index_bytes).expect("write test index");

        let index = SpecialistIndex::open(index_path.to_str().unwrap()).expect("open index");

        assert_eq!(index.predict_fraud_count(&[0i16; PACKED_DIMS]), 0);
        assert_eq!(index.predict_fraud_count(&[SCALE; PACKED_DIMS]), 5);

        std::fs::remove_file(index_path).ok();
    }

    #[test]
    fn restricted_partition_search_uses_only_allowed_keys() {
        let index_path = std::env::temp_dir().join(format!(
            "rinha-rust-tree-restricted-index-{}-{}.idx",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let mut references = Vec::new();
        for _ in 0..8 {
            let vector = [0i16; PACKED_DIMS];
            references.push(Reference { vector, label: 0 });
        }
        for _ in 0..8 {
            let mut vector = [SCALE; PACKED_DIMS];
            vector[4] = SCALE;
            references.push(Reference { vector, label: 1 });
        }

        let index_bytes =
            build_index(references, 8, PartitionScheme::recommended()).expect("build index");
        std::fs::write(&index_path, index_bytes).expect("write test index");

        let index = SpecialistIndex::open(index_path.to_str().unwrap()).expect("open index");
        let high_query = [SCALE; PACKED_DIMS];
        let mut low_query = [0i16; PACKED_DIMS];
        low_query[4] = 0;

        let mut high_allowed = PartitionSet::empty();
        high_allowed.set(index.compute_partition_key(&high_query));
        let mut low_allowed = PartitionSet::empty();
        low_allowed.set(index.compute_partition_key(&low_query));

        assert_eq!(
            index.predict_fraud_count_in_partitions(&high_query, &high_allowed),
            5
        );
        assert_eq!(
            index.predict_fraud_count_in_partitions(&high_query, &low_allowed),
            0
        );

        std::fs::remove_file(index_path).ok();
    }

    #[test]
    fn tree_key_walks_heap_order_predicates() {
        let predicates = [
            TreePredicate {
                dim: 0,
                threshold: 10,
                enabled: true,
            },
            TreePredicate {
                dim: 1,
                threshold: 10,
                enabled: true,
            },
            TreePredicate {
                dim: 2,
                threshold: 10,
                enabled: true,
            },
        ];
        let mut vector = [0i16; PACKED_DIMS];
        vector[0] = 11;
        vector[2] = 11;

        assert_eq!(compute_tree_key(&vector, 2, &predicates), 0b11);
    }

    #[test]
    fn tree256_index_roundtrip_predicts_nearest_labels() {
        let index_path = std::env::temp_dir().join(format!(
            "rinha-rust-tree-tree256-index-{}-{}.idx",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let mut references = Vec::new();
        for offset in 0..32 {
            let mut vector = [0i16; PACKED_DIMS];
            vector[0] = offset;
            references.push(Reference { vector, label: 0 });
        }
        for offset in 0..32 {
            let mut vector = [SCALE; PACKED_DIMS];
            vector[0] = SCALE - offset;
            references.push(Reference { vector, label: 1 });
        }

        let index_bytes = build_index(references, 8, PartitionScheme::by_name("tree256").unwrap())
            .expect("build index");
        std::fs::write(&index_path, index_bytes).expect("write test index");

        let index = SpecialistIndex::open(index_path.to_str().unwrap()).expect("open index");

        assert_eq!(index.predict_fraud_count(&[0i16; PACKED_DIMS]), 0);
        assert_eq!(index.predict_fraud_count(&[SCALE; PACKED_DIMS]), 5);

        std::fs::remove_file(index_path).ok();
    }
}
