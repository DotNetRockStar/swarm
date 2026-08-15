"""Typed mirror of the Overmind request/response contracts the Drone talks to.

The Drone is the *client* in these exchanges: it POSTs registration/heartbeat/telemetry to
Overmind and reads a small set of fields back. These Pydantic models mirror Overmind's
`src/overmind/models.py` so we can (a) document the wire contract on the Drone side and
(b) assert in tests that the payloads the Drone builds still validate against what Overmind
accepts, and that the response fields the Drone reads are part of Overmind's response models.

IMPORTANT: this module imports pydantic and is therefore **not** imported by the stdlib-only
on-device runtime path. It is used by tests and (later) by the vendored FastAPI layer only.
Keep it in sync with batocera.overmind/src/overmind/models.py.
"""

from typing import Any, Optional

from pydantic import BaseModel, ConfigDict, Field


class StrictContractModel(BaseModel):
    """extra=forbid — mirrors Overmind's strict drone-facing request contracts."""
    model_config = ConfigDict(extra="forbid")


class ExtensibleContractModel(BaseModel):
    """extra=allow — mirrors Overmind's versioned/extensible records."""
    model_config = ConfigDict(extra="allow")


# ---- Nested payload fragments (permissive, like Overmind) ----

class BatoceraInfoContract(ExtensibleContractModel):
    model: str
    system: str
    architecture: str
    cpu_model: str
    cpu_cores: int
    cpu_threads: int
    cpu_max_frequency: str
    memory_available: str
    memory_total: str
    ip_address: str
    network: Optional[dict] = None
    api_port: Optional[int] = None
    scheme: Optional[str] = None
    reachable_url: Optional[str] = None
    certificate: Optional[dict] = None
    system_info: Optional[dict] = None


# ---- Outbound requests (Drone -> Overmind) ----

class DeviceRegisterContract(BaseModel):
    """Mirror of Overmind DeviceRegister (POST /api/devices/register)."""
    email: Optional[str] = None
    password: Optional[str] = None
    authorization_token: Optional[str] = None
    device_id: str
    device_name: str
    batocera_info: BatoceraInfoContract
    api_port: Optional[int] = None
    scheme: Optional[str] = None
    reachable_url: Optional[str] = None


class DroneHeartbeatContract(StrictContractModel):
    """Mirror of Overmind DroneHeartbeatRequest (strict: extra keys are rejected with 422)."""
    device_id: Optional[str] = None
    device_name: Optional[str] = None
    network: Optional[dict] = None
    api_port: Optional[int] = None
    scheme: Optional[str] = None
    protocol: Optional[str] = None
    reachable_url: Optional[str] = None
    certificate: Optional[dict] = None
    system_info: Optional[dict] = None
    downloads: Optional[dict] = None
    rom_inventory_fingerprint: Optional[str] = None
    rom_inventory_fingerprint_algorithm: Optional[str] = None
    romset_files_thumbprint: Optional[str] = None
    bios_files_thumbprint: Optional[str] = None
    saves_files_thumbprint: Optional[str] = None
    rom_metadata: Optional[dict] = None
    rom_systems: list[dict] = Field(default_factory=list)


# ---- Inbound responses (Overmind -> Drone) the Drone reads ----

class HeartbeatResponseContract(ExtensibleContractModel):
    """Fields the Drone reads from the heartbeat response (drone_api.py heartbeat loop)."""
    actions: list[Any] = Field(default_factory=list)
    swarm: list[Any] = Field(default_factory=list)
    log_stream_requested: bool = False
    romset_files_thumbprint: Optional[str] = None
    bios_files_thumbprint: Optional[str] = None


class DeviceRegisterResponseContract(ExtensibleContractModel):
    """Fields the Drone reads from the register/claim response."""
    message: Optional[str] = None
    status: Optional[str] = None
    device_id: Optional[str] = None
    drone_token: Optional[str] = None
