import { computed, reactive } from 'vue'
import { defineStore } from 'pinia'
import { invoke } from '@tauri-apps/api/core'
import {
  CLI_DESCRIPTORS,
  CLI_KINDS,
  getCliIssueDefinition,
  type CliKind,
  type CliStatus,
} from '@/types/cli'
import { InFlightTaskCache } from '@/utils/inFlightTaskCache'

type CliStatusMap = Record<CliKind, CliStatus | null>

function initialStatuses(): CliStatusMap {
  return { claude: null, codex: null, opencode: null, dsh: null }
}

export const useCliRuntimeStore = defineStore('cliRuntime', () => {
  const statuses = reactive<CliStatusMap>(initialStatuses())
  const checking = reactive<Record<CliKind, boolean>>({
    claude: false,
    codex: false,
    opencode: false,
    dsh: false,
  })
  const inFlight = new InFlightTaskCache<CliKind, CliStatus>()

  const readyKinds = computed(() =>
    CLI_KINDS.filter((kind) => statuses[kind]?.state === 'ready'),
  )

  async function check(kind: CliKind, force = false): Promise<CliStatus> {
    const cached = statuses[kind]
    if (!force && cached && cached.state !== 'checking') return cached
    const existing = inFlight.get(kind)
    if (existing) return existing

    checking[kind] = true
    // 后台补齐（kicked-off 与轮询）不能把 UI 打回“正在检查”：已有结论
    // （哪怕是 blocked）就保留原状，只在没有结论时才摆出 checking 占位。
    const silent = cached != null
    if (!silent) statuses[kind] = checkingStatus(kind)
    return inFlight.run(kind, async () => {
      try {
        const status = await invoke<CliStatus>('check_cli', { kind })
        statuses[kind] = status
        return status
      } catch (error) {
        const definition = getCliIssueDefinition('version_command_failed')
        const status: CliStatus = {
          kind,
          state: definition.state,
          issueCode: definition.code,
          message: `${definition.messages[kind]} ${String(error)}`,
          executablePath: null,
          version: null,
        }
        statuses[kind] = status
        return status
      } finally {
        checking[kind] = false
      }
    })
  }

  function checkingStatus(kind: CliKind): CliStatus {
    return {
      kind,
      state: 'checking',
      issueCode: null,
      message: `正在检查 ${CLI_DESCRIPTORS[kind].label}。`,
      executablePath: statuses[kind]?.executablePath ?? null,
      version: statuses[kind]?.version ?? null,
    }
  }

  function executable(kind: CliKind): string {
    return statuses[kind]?.executablePath || CLI_DESCRIPTORS[kind].command
  }

  return {
    statuses,
    checking,
    readyKinds,
    check,
    executable,
  }
})
