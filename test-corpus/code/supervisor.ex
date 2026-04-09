defmodule Atlas.Pipeline.Supervisor do
  @moduledoc """
  Atlas pipeline supervisor.

  Starts and monitors the ingestion, transformation, and storage workers
  under an OTP supervision tree.  Uses a :rest_for_one strategy so that
  if the ingestion worker crashes, the transform and storage workers are
  also restarted (they depend on the ingestion queue being alive).
  """

  use Supervisor

  # Maximum restarts in the restart_window before the supervisor itself crashes.
  @max_restarts 5
  @restart_window_seconds 10

  # Type alias expressed as a module attribute for documentation purposes.
  @type pipeline_id :: String.t()

  # ───────────────────────────────────────────────────────────────────────────
  # Public API
  # ───────────────────────────────────────────────────────────────────────────

  @doc """
  Start the pipeline supervisor linked to the calling process.

  ## Options

    * `:pipeline_id` - unique identifier for this pipeline instance (required)
    * `:buffer_size` - ingestion queue depth, default #{4096}
    * `:flush_interval_ms` - storage flush cadence in ms, default #{5_000}

  ## WHY

  We accept options as a keyword list rather than a struct to stay consistent
  with OTP conventions and avoid coupling callers to internal types.
  """
  @spec start_link(keyword()) :: Supervisor.on_start()
  def start_link(opts) do
    Supervisor.start_link(__MODULE__, opts, name: via(Keyword.fetch!(opts, :pipeline_id)))
  end

  @doc "Return the child specification for embedding in a parent supervisor."
  def child_spec(opts) do
    %{
      id: __MODULE__,
      start: {__MODULE__, :start_link, [opts]},
      type: :supervisor,
      restart: :permanent,
      shutdown: :infinity
    }
  end

  # ───────────────────────────────────────────────────────────────────────────
  # Supervisor callbacks
  # ───────────────────────────────────────────────────────────────────────────

  @impl Supervisor
  def init(opts) do
    pipeline_id = Keyword.fetch!(opts, :pipeline_id)
    buffer_size = Keyword.get(opts, :buffer_size, 4096)
    flush_ms    = Keyword.get(opts, :flush_interval_ms, 5_000)

    children = [
      {Atlas.Ingestion.Worker,  [pipeline_id: pipeline_id, buffer_size: buffer_size]},
      {Atlas.Transform.Worker,  [pipeline_id: pipeline_id]},
      {Atlas.Storage.Worker,    [pipeline_id: pipeline_id, flush_interval_ms: flush_ms]},
      {Atlas.Metrics.Collector, [stage: pipeline_id]},
    ]

    Supervisor.init(children,
      strategy: :rest_for_one,
      max_restarts: @max_restarts,
      max_seconds: @restart_window_seconds
    )
  end

  # ───────────────────────────────────────────────────────────────────────────
  # Private helpers
  # ───────────────────────────────────────────────────────────────────────────

  defp via(pipeline_id), do: {:via, Registry, {Atlas.PipelineRegistry, pipeline_id}}
end

defmodule Atlas.Ingestion.Worker do
  @moduledoc """
  GenServer that receives events from HTTP/Kafka and enqueues them for
  the transform worker via an in-process message queue.
  """

  use GenServer, restart: :permanent

  require Logger

  # NOTE: we pattern-match on `{:ingest, event}` tuples so the GenServer
  # mailbox doubles as a typed API — no additional routing layer needed.

  @spec start_link(keyword()) :: GenServer.on_start()
  def start_link(opts) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @doc "Enqueue a raw event map for asynchronous processing."
  @spec enqueue(map()) :: :ok
  def enqueue(event), do: GenServer.cast(__MODULE__, {:ingest, event})

  # ── GenServer callbacks ───────────────────────────────────────────────────

  @impl GenServer
  def init(opts) do
    buffer_size = Keyword.get(opts, :buffer_size, 4096)
    Logger.info("ingestion worker starting, buffer_size=#{buffer_size}")
    {:ok, %{queue: :queue.new(), capacity: buffer_size, dropped: 0}}
  end

  @impl GenServer
  def handle_cast({:ingest, event}, state) do
    if :queue.len(state.queue) >= state.capacity do
      # HACK: drop the oldest event when the queue is full to prevent the
      # mailbox from growing unbounded.  A proper implementation would apply
      # back-pressure to the HTTP listener.  Tracked in #318.
      {_dropped, q2} = :queue.out(state.queue)
      new_state = %{state | queue: :queue.in(event, q2), dropped: state.dropped + 1}
      {:noreply, new_state}
    else
      {:noreply, %{state | queue: :queue.in(event, state.queue)}}
    end
  end

  @impl GenServer
  def handle_call(:drain, _from, state) do
    events = :queue.to_list(state.queue)
    {:reply, events, %{state | queue: :queue.new()}}
  end

  @impl GenServer
  def handle_info(:tick, state) do
    events = :queue.to_list(state.queue)

    enriched =
      events
      |> Enum.map(&Atlas.Ingestion.Worker.enrich_event/1)
      |> Enum.filter(&valid_event?/1)

    Enum.each(enriched, &Atlas.Transform.Worker.submit/1)
    {:noreply, %{state | queue: :queue.new()}}
  end

  # ── Private helpers ───────────────────────────────────────────────────────

  defp enrich_event(event) do
    event
    |> Map.put_new("id", UUID.uuid4())
    |> Map.put("ingested_at", DateTime.utc_now() |> DateTime.to_iso8601())
  end

  defp valid_event?(%{"source" => source}) when is_binary(source) and source != "", do: true
  defp valid_event?(_), do: false
end
