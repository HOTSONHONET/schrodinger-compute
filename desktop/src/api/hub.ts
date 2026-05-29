const HUB_URL = "http://127.0.0.1:8001";

export async function getAgents() {
    const res = await fetch(`${HUB_URL}/v1/agents`);

    if (!res.ok) {
        throw new Error("failed to fetch agents");
    }

    return res.json();
}