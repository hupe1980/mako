// Every test runs in a zone that is not Berlin, so a date that is only right
// because the developer sits in Germany fails here.
process.env.TZ = "America/New_York";
