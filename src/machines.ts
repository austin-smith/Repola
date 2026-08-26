import { invoke } from "@tauri-apps/api/core";
import type { AgentInfo, MachineProfile, MachineProfileInput } from "./types";
import { invokeOperation } from "./operations";

export function loadMachines(): Promise<MachineProfile[]> {
  return invoke<MachineProfile[]>("load_machines");
}

export function saveMachine(input: MachineProfileInput): Promise<MachineProfile[]> {
  return invoke<MachineProfile[]>("upsert_machine", { input });
}

export function deleteMachine(machineId: string): Promise<MachineProfile[]> {
  return invoke<MachineProfile[]>("remove_machine", { machineId });
}

export function moveMachine(machineId: string, delta: -1 | 1): Promise<MachineProfile[]> {
  return invoke<MachineProfile[]>("move_machine", { machineId, delta });
}

export function testMachine(machineId: string): Promise<AgentInfo> {
  return invokeOperation<AgentInfo>("test_machine", { machineId });
}
