// Default MaKo authorization policy.
//
// Baseline: every named (authenticated) principal is permitted to perform any
// MaKo action. This replicates the previous shared-token model while
// introducing named principal identities that appear in audit logs.
//
// Layer additional policies on top via `--cedar-policy-dir` to implement
// fine-grained ABAC rules. Because the catch-all below already permits
// everything, only `forbid` statements narrow it — an added `permit` grants
// nothing new. To build up from a deny-all baseline instead, start makod with
// `--cedar-no-default-policy`, which omits this file entirely and makes the
// policy directory the only source of access (see conservative.cedar).
//
// Examples, all in the layer-on-top form:
//
//   // Example: restrict "ci-readonly" to read-only MaLo access
//   forbid(
//     principal == MaKo::Principal::"ci-readonly",
//     action in [
//       MaKo::Action::"AdminMaloWrite",
//       MaKo::Action::"AdminMaloDelete",
//       MaKo::Action::"SubmitCommand",
//       MaKo::Action::"IngestEdifact"
//     ],
//     resource
//   );
//
//   // Example: restrict "erp-gas" to gas-only commands
//   forbid(
//     principal == MaKo::Principal::"erp-gas",
//     action    == MaKo::Action::"SubmitCommand",
//     resource  is MaKo::Command
//   )
//   unless {
//     resource.marktrolle == "LFG" || resource.marktrolle == "GNB"
//   };
//
//   // Example: restrict a key to a specific tenant
//   forbid(
//     principal == MaKo::Principal::"erp-tenant-a",
//     action,
//     resource
//   )
//   unless {
//     resource has tenant && resource.tenant == "9900357000001"
//   };
//
//   // Example: grant read-only access using action groups
//   // (AdminMalo groups AdminMaloRead/Write/Delete/Stats;
//   //  AdminPartner groups AdminPartnerRead/Write/Delete/Import)
//   forbid(
//     principal == MaKo::Principal::"grafana-ro",
//     action in [MaKo::Action::"AdminMalo", MaKo::Action::"AdminPartner"],
//     resource
//   )
//   unless {
//     action == MaKo::Action::"AdminMaloStats"
//     || action == MaKo::Action::"AdminPartnerRead"
//   };

permit(
  principal is MaKo::Principal,
  action,
  resource
);
