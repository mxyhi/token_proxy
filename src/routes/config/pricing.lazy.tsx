import { createLazyFileRoute } from "@tanstack/react-router";

import { ModelPricingPage } from "@/features/pricing/pages/model-pricing-page";

export const Route = createLazyFileRoute("/config/pricing")({
  component: PricingRoute,
});

function PricingRoute() {
  return <ModelPricingPage />;
}
