import { useEffect, useState } from "react";
import { getAgents } from "./api/hub";
import "./App.css";

type Agent = {
  id: string;
  url: string;
  status: string;
  last_seen?: string;
  resources?: {
    cpu_cores?: number;
    memory_total_mb?: number;
    memory_used_mb?: number;
    disk_total_mb?: number;
    disk_free_mb?: number;
    gpu?: {
      name?: string;
      memory_total_mb?: number;
      memory_used_mb?: number;
      utilization_percent?: number;
      temperature_c?: number;
      power_draw_w?: number;
    };
  };
};

function percent(used?: number, total?: number) {
  if (!used || !total) return 0;
  return Math.round((used / total) * 100);
}

function App() {
  const [agents, setAgents] = useState<Agent[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    async function load() {
      try {
        const data = await getAgents();
        setAgents(data);
      } catch (err) {
        console.error(err);
      } finally {
        setLoading(false);
      }
    }

    load();
    const interval = setInterval(load, 3000);
    return () => clearInterval(interval);
  }, []);

  return (
    <main className="page">
      <header className="header">
        <div>
          <h1>Schrodinger Compute</h1>
          <p>Distributed compute node dashboard</p>
        </div>
        <div className="badge">{agents.length} node(s)</div>
      </header>

      <section className="grid">
        {loading ? (
          <p>Loading nodes...</p>
        ) : (
          agents.map((agent) => {
            const r = agent.resources;
            const gpu = r?.gpu?.gpus?.[0];
            const ramTotal = r?.ram_total_mb ?? 0;
            const ramFree = r?.ram_free_mb ?? 0;
            const ramUsed = ramTotal - ramFree;

            const ramPct = percent(ramUsed, ramTotal);

            const gpuTotal = gpu?.memory_total_mib ?? 0;
            const gpuUsed = gpu?.memory_used_mib ?? 0;

            const gpuPct = percent(gpuUsed, gpuTotal);

            return (
              <article className="card" key={agent.id}>
                <div className="cardTop">
                  <div>
                    <h2>{agent.id}</h2>
                    <p>{agent.url}</p>
                  </div>
                  <span className={agent.status === "UP" ? "status up" : "status down"}>
                    {agent.status}
                  </span>
                </div>

                <div className="stats">
                  <div>
                    <span>CPU</span>
                    <strong>{r?.cpu_cores ?? "-"} cores</strong>
                  </div>
                  <div>
                    <span>RAM</span>
                    <strong>{ramPct}%</strong>
                  </div>
                  <div>
                    <span>GPU</span>
                    <strong>{gpu?.name ?? "No GPU"}</strong>
                  </div>
                </div>

                <div className="metric">
                  <div className="metricLabel">
                    <span>Memory</span>
                    <span>{ramUsed} / {ramTotal} MB</span>
                  </div>
                  <div className="bar">
                    <div style={{ width: `${ramPct}%` }} />
                  </div>
                </div>

                {gpu && (
                  <div className="metric">
                    <div className="metricLabel">
                      <span>GPU VRAM</span>
                      <span>{gpuUsed} / {gpuTotal} MiB</span>
                    </div>
                    <div className="bar">
                      <div style={{ width: `${gpuPct}%` }} />
                    </div>

                    <div className="gpuDetails">
                      <span>Util: {gpu.utilization_gpu_pct}%</span>
                      <span>Temp: {gpu.temperature_c}°C</span>
                      <span>Power: {gpu.power_draw_wx}W</span>
                    </div>
                  </div>
                )}

                <footer>
                  Last seen: {agent.last_seen ?? "unknown"}
                </footer>
              </article>
            );
          })
        )}
      </section>
    </main>
  );
}

export default App;