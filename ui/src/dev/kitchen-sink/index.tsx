import { AnalysisSection } from "./analysis-section";
import { D11Section } from "./d11-section";
import { HistorySection } from "./history-section";
import { PrimitivesSection } from "./primitives-section";
import { QueueComponentsSection } from "./queue-components-section";
import { QueueSection } from "./queue-section";
import { TokensSection } from "./tokens-section";

/**
 * Dev-only component workshop (#36 D10). Excluded from release bundles via
 * the import.meta.env.DEV gate in App.tsx — Vite eliminates the dynamic
 * import entirely, so no chunk is emitted.
 */
export default function KitchenSink() {
  return (
    <div className="flex flex-col gap-8 p-6">
      <h1 className="text-2xl">Kitchen sink</h1>
      <TokensSection />
      <PrimitivesSection />
      <D11Section />
      <QueueSection />
      <QueueComponentsSection />
      <AnalysisSection />
      <HistorySection />
    </div>
  );
}
