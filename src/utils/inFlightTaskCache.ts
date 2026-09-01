export class InFlightTaskCache<Key, Value> {
  private readonly tasks = new Map<Key, Promise<Value>>()

  get(key: Key) {
    return this.tasks.get(key)
  }

  run(key: Key, task: () => Promise<Value>) {
    const existing = this.tasks.get(key)
    if (existing) return existing

    const request = Promise.resolve()
      .then(task)
      .finally(() => {
        if (this.tasks.get(key) === request) this.tasks.delete(key)
      })
    this.tasks.set(key, request)
    return request
  }
}

