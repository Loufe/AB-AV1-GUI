import {
  commands,
  type AnalysisGenerationId,
  type AnalysisQueueRequest,
  type AnalysisQueueSummary,
} from "@/lib/bindings";

export async function discoverAnalysis(roots: string[]): Promise<AnalysisGenerationId> {
  const result = await commands.analysisDiscover(roots);
  if (result.status === "error") {
    throw new Error(`Discovery failed (${result.error.code}): ${result.error.message}`);
  }
  return result.data;
}

export async function basicScanAnalysis(generation: AnalysisGenerationId): Promise<void> {
  const result = await commands.analysisBasicScan(generation);
  if (result.status === "error") {
    throw new Error(`Basic Scan failed (${result.error.code}): ${result.error.message}`);
  }
}

export async function cancelAnalysis(): Promise<void> {
  const result = await commands.analysisCancel();
  if (result.status === "error") {
    throw new Error(`Cancel failed (${result.error.code}): ${result.error.message}`);
  }
}

export async function enqueueAnalysis(
  request: AnalysisQueueRequest,
): Promise<AnalysisQueueSummary> {
  const result = await commands.analysisEnqueue(request);
  if (result.status === "error") {
    throw new Error(`Queue action failed (${result.error.code}): ${result.error.message}`);
  }
  return result.data;
}
