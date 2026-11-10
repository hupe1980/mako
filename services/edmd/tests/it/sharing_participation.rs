//! Which § 42c / § 42b community a Marktlokation takes part in — the lookup
//! `GET /api/v1/sharing/participants/{malo_id}/allocation` resolves before it
//! allocates, against real PostgreSQL.

use edmd::domain::virtual_meter::VirtualMeterRule;
use metering::allocation::community::AllocationKey;

/// **Invariant: a MaLo is a participant only where it sources the rule's
/// tenant.**
///
/// The plant's MaLo is named in every participant's `sources`, so a match on
/// any source would make the plant a participant of its own community and
/// credit it an allocation. A MaLo two communities name is reported for both,
/// so the endpoint can refuse rather than pick one.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn a_participant_is_found_by_its_own_malo_and_not_by_the_plants() {
    let (_repo, pool, _pg, _wh) = crate::meterstore_integration::setup().await;
    let tenant = "9910000000010";
    let melo = |n: u8| -> metering::MeloId {
        format!("DE00012345678900000000000000000{n:02}")
            .parse()
            .expect("a MeLo")
    };
    let malo = |id: &str| -> metering::MaloId { id.parse().expect("a MaLo") };
    let plant_malo = malo("51238696129");
    let tenant_a = malo("51238696012");
    let tenant_b = malo("51238696020");

    for (vid, tenant_melo, tenant_malo, plant) in [
        ("VIRT-A", melo(2), tenant_a, melo(1)),
        ("VIRT-B", melo(3), tenant_b, melo(1)),
        // The same MaLo in a second community.
        ("VIRT-A2", melo(2), tenant_a, melo(11)),
    ] {
        let rule = VirtualMeterRule::CommunityShare {
            plant,
            tenant: tenant_melo,
            key: AllocationKey::EqualShares,
            sources: [(plant, plant_malo), (tenant_melo, tenant_malo)]
                .into_iter()
                .collect(),
        };
        sqlx::query(
            "INSERT INTO virtual_meter_configs
               (virtual_malo_id, display_name, rule_type, rule_json, sparte, valid_from, tenant)
             VALUES ($1,$1,$2,$3,'STROM',heute(),$4)",
        )
        .bind(vid)
        .bind(rule.kind())
        .bind(serde_json::to_value(&rule).expect("a rule serialises"))
        .bind(tenant)
        .execute(&pool)
        .await
        .expect("insert the rule");
    }

    let of = |m| {
        let pool = pool.clone();
        async move {
            edmd::server::community_participations(&pool, tenant, &m)
                .await
                .expect("lookup")
        }
    };
    assert_eq!(of(tenant_b).await, vec![(melo(1), melo(3))]);
    assert_eq!(
        of(tenant_a).await,
        vec![(melo(1), melo(2)), (melo(11), melo(2))],
        "a MaLo in two communities is reported in both"
    );
    assert!(
        of(plant_malo).await.is_empty(),
        "the plant's MaLo is no participant"
    );
    assert!(
        edmd::server::community_participations(&pool, "9910000000099", &tenant_b)
            .await
            .expect("lookup")
            .is_empty(),
        "another tenant's rules are not this tenant's communities"
    );
}
