import interference from "../assets/onboarding-interference.png";

/** Original high-resolution halftone artwork, redrawn from the two print references. */
export function OverviewBackdrop() {
  return <div className="overview-backdrop" aria-hidden="true">
    <img src={interference} alt="" width={2172} height={724} />
  </div>;
}
