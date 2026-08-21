"""The CloudEvents half of the observable contract.

EDIFACT is one wire contract a MaKo platform exposes; the event stream is the
other. The trap it carries is the mirror image of vacuous validation: a test
that names a type the platform does not declare — a typo, or one retired by a
rename — passes forever as "no such event was emitted", and *that* is the
direction a missing-event assertion is looking in.
"""

import pytest

from makotest import (
    assert_cloudevent,
    assert_event_emitted,
    assert_no_event_emitted,
    cloudevent_core_attributes,
    cloudevent_json_members,
    event_matches,
    event_type_exists,
    event_types,
    event_types_matching,
    find_events,
    is_valid_extension_key,
    parse_cloudevent_time,
)

MALO = "51238696012"


def event(type_: str, *, subject: str = MALO, **extra) -> dict:
    return {
        "specversion": "1.0",
        "id": "01234567-89ab-cdef-0123-456789abcdef",
        "source": "https://mako.example/makod",
        "type": type_,
        "time": "2026-03-02T09:00:00Z",
        "subject": subject,
        "data": {},
        **extra,
    }


class TestCatalog:
    def test_the_catalog_is_bound_not_copied(self):
        types = event_types()
        assert len(types) > 50
        assert types == sorted(set(types))
        assert "de.mako.process.completed" in types

    def test_a_retired_prefix_is_not_a_type(self):
        """The rename is why the catalog is bound.

        `de.edmd.*` became `de.messwert.*`. A test still naming the old prefix
        asserts that an event nobody emits was not emitted — which is true, and
        useless.
        """
        assert event_type_exists("de.messwert.reading.direct.stored")
        assert not event_type_exists("de.edmd.reading.direct.stored")

    def test_a_typo_is_not_a_type(self):
        assert not event_type_exists("de.mako.process.complete")

    def test_a_pattern_resolves_to_what_it_would_deliver(self):
        mako = event_types_matching("de.mako.*")
        assert "de.mako.process.initiated" in mako
        assert not any(t.startswith("de.markt.") for t in mako)
        assert event_types_matching("*") == event_types()

    def test_a_dead_subscription_is_visible_as_one(self):
        assert event_types_matching("de.nosuch.*") == []

    def test_the_matcher_is_the_platforms_own(self):
        """`*` is any sequence and `?` exactly one character — not `startswith`."""
        assert event_matches("de.*.rechnung.*", "de.billing.rechnung.erstellt")
        assert event_matches("*", "de.anything")
        assert not event_matches("de.mako.*", "de.markt.malo.updated")


class TestEnvelope:
    def test_a_conformant_event_passes(self):
        assert_cloudevent(
            event("de.mako.process.completed"), type="de.mako.process.completed"
        )

    def test_a_missing_required_attribute_is_reported(self):
        broken = event("de.mako.process.completed")
        del broken["source"]
        with pytest.raises(AssertionError, match="required attribute"):
            assert_cloudevent(broken)

    def test_a_wrong_specversion_is_refused(self):
        with pytest.raises(AssertionError, match="specversion"):
            assert_cloudevent(event("de.mako.process.completed", specversion="0.3"))

    def test_a_non_rfc3339_time_is_refused(self):
        """`OffsetDateTime::to_string()` is not RFC 3339, and it renders plausibly."""
        with pytest.raises(ValueError, match="RFC 3339"):
            assert_cloudevent(
                event("de.mako.process.completed", time="2026-03-02 9:00:00.0 +00:00:00")
            )

    def test_an_undeclared_type_is_refused_even_when_the_envelope_is_fine(self):
        with pytest.raises(AssertionError, match="not a type the platform declares"):
            assert_cloudevent(event("de.mako.process.invented"))

    def test_an_extension_key_colliding_with_a_core_attribute_is_refused(self):
        """Serialised flat, a collision emits the key twice and receivers reject it."""
        assert "data" in cloudevent_core_attributes()
        assert not is_valid_extension_key("data")

    @pytest.mark.parametrize("key", ["makoPid", "mako-pid", "mako_pid", ""])
    def test_illegal_extension_keys_are_refused(self, key):
        """§3.3 allows lowercase letters and digits only."""
        assert not is_valid_extension_key(key)
        if key:
            with pytest.raises(AssertionError, match="extension attribute"):
                assert_cloudevent(event("de.mako.process.completed", **{key: "x"}))

    def test_legal_extensions_pass(self):
        assert is_valid_extension_key("makopid") and is_valid_extension_key("traceparent")
        assert_cloudevent(event("de.mako.process.completed", makopid="55001"))

    def test_a_binary_payload_is_conformant(self):
        """`data_base64` is a JSON-format member, not a context attribute.

        Its underscore makes it an illegal *extension* name, so an envelope
        check that knew only the core nine would reject a conformant event that
        happens to carry bytes.
        """
        binary = event("de.mako.process.completed")
        del binary["data"]
        binary["data_base64"] = "VU5CKw=="
        assert_cloudevent(binary)
        assert "data_base64" in cloudevent_json_members()
        assert not is_valid_extension_key("data_base64")

    def test_carrying_both_payload_members_is_refused(self):
        """§3.1 makes them mutually exclusive — two payloads, no rule for which."""
        with pytest.raises(AssertionError, match="both `data` and `data_base64`"):
            assert_cloudevent(event("de.mako.process.completed", data_base64="VU5CKw=="))

    def test_the_time_helper_normalises(self):
        assert parse_cloudevent_time("2026-03-02T09:00:00Z") == "2026-03-02T09:00:00Z"


class TestEmission:
    @pytest.fixture
    def emitted(self) -> list[dict]:
        return [
            event("de.mako.process.initiated"),
            event("de.markt.versorgung.changed"),
            event("de.mako.process.completed", subject="51238696780"),
        ]

    def test_finding_by_pattern_uses_the_platforms_routing(self, emitted):
        assert len(find_events(emitted, "de.mako.*")) == 2
        assert len(find_events(emitted, "de.mako.*", subject=MALO)) == 1

    def test_assert_event_emitted_returns_the_match(self, emitted):
        found = assert_event_emitted(emitted, "de.markt.versorgung.*", subject=MALO)
        assert found["type"] == "de.markt.versorgung.changed"

    def test_a_missing_event_lists_what_was_seen(self, emitted):
        with pytest.raises(AssertionError, match="Types seen"):
            assert_event_emitted(emitted, "de.billing.*")

    def test_assert_no_event_emitted(self, emitted):
        assert_no_event_emitted(emitted, "de.billing.*")
        with pytest.raises(AssertionError, match="but 2 was/were emitted"):
            assert_no_event_emitted(emitted, "de.mako.*")

    def test_a_pattern_the_catalog_cannot_satisfy_refuses_to_filter(self, emitted):
        """Otherwise `assert_no_event_emitted` passes on a typo, forever."""
        with pytest.raises(ValueError, match="would be dead"):
            assert_no_event_emitted(emitted, "de.edmd.*")
        with pytest.raises(ValueError, match="would be dead"):
            find_events(emitted, "de.mako.proces.*")
