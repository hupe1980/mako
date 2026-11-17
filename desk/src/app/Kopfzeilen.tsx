import { Drift } from "./Drift";
import { Unbestaetigt } from "./Unbestaetigt";

/**
 * The lines under the top bar that wait for the first paint: the drift
 * banner and the follow-up on decisions the last page sent as it closed. One
 * lazy module, so the first paint carries neither.
 */
export function Kopfzeilen() {
  return (
    <>
      <Drift />
      <Unbestaetigt />
    </>
  );
}
