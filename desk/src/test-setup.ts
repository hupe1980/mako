// Every test runs in a zone that is not Berlin, so a date that is only right
// because the developer sits in Germany fails here.
process.env.TZ = "America/New_York";

// The English catalogue loads on demand in the app; tests switch Sprache
// synchronously, so it is loaded before any of them runs.
import { ladeSprache } from "./i18n";

await ladeSprache("en");
