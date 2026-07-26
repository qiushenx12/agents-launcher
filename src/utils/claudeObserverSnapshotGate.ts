export interface ClaudeObserverSnapshotRequest {
  tabId: number
  requestId: number
  liveGeneration: number
}

export class ClaudeObserverSnapshotGate {
  private readonly liveGenerations = new Map<number, number>()
  private readonly requestIds = new Map<number, number>()

  markLiveStatus(tabId: number) {
    this.liveGenerations.set(tabId, (this.liveGenerations.get(tabId) ?? 0) + 1)
  }

  beginSnapshot(tabId: number): ClaudeObserverSnapshotRequest {
    const requestId = (this.requestIds.get(tabId) ?? 0) + 1
    this.requestIds.set(tabId, requestId)
    return {
      tabId,
      requestId,
      liveGeneration: this.liveGenerations.get(tabId) ?? 0,
    }
  }

  isLatestRequest(request: ClaudeObserverSnapshotRequest) {
    return this.requestIds.get(request.tabId) === request.requestId
  }

  canApplyStatus(request: ClaudeObserverSnapshotRequest) {
    return this.isLatestRequest(request)
      && (this.liveGenerations.get(request.tabId) ?? 0) === request.liveGeneration
  }

  clear(tabId: number) {
    this.liveGenerations.delete(tabId)
    this.requestIds.delete(tabId)
  }
}
