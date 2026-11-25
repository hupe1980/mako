// Each review installation holds one Strom NB cohort. Its named grants further
// restrict these two actions to the subjects appointed for that cohort.
permit (
    principal,
    action in [Action::"read-shadow-review", Action::"resolve-shadow-review"],
    resource
)
when {
    context.principal_tenant == context.resource_tenant &&
    context.principal_roles.contains("NB")
};
