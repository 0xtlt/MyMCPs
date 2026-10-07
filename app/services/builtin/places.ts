/**
 * Places for work that keeps a whole file in memory or a connection open,
 * counted for each MCP so that one cannot use up the instance.
 *
 * The returned function takes one of the MCP's places. It returns how to give
 * the place back, or `null` when all are taken.
 */
export function limitedPlaces(max: number) {
  const taken = new Map<number, number>()

  return (mcpId: number) => {
    const running = taken.get(mcpId) ?? 0
    if (running >= max) return null

    taken.set(mcpId, running + 1)
    return () => {
      const left = (taken.get(mcpId) ?? 1) - 1
      if (left > 0) taken.set(mcpId, left)
      else taken.delete(mcpId)
    }
  }
}
