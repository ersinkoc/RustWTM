using System.Security.Principal;
using System.Text.Json;
using LibreHardwareMonitor.Hardware;

namespace Wtm.Hardware;

internal sealed record Reading(string Id, string Device, string Name, string Kind, float? Value);
internal sealed record Channel(string Id, string Device, string Name, float MinPercent, float MaxPercent, bool Writable, string Reason);
internal sealed record Report(string Provider, long SampledAtMs, List<Reading> Sensors, List<Channel> Channels, string Notice);

internal static class Program
{
    private static readonly JsonSerializerOptions Json = new() { PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower };
    private const int MaxRequest = 64 * 1024;
    private static readonly object Gate = new();
    private static readonly Leases Leases = new(() => Environment.TickCount64);
    private static readonly Dictionary<string, IControl> Controls = [];
    private static readonly HashSet<string> Allowed = [];

    private static async Task<int> Main(string[] args)
    {
        if (args.Contains("--self-test")) { Leases.Test(); return 0; }
        bool admin = new WindowsPrincipal(WindowsIdentity.GetCurrent()).IsInRole(WindowsBuiltInRole.Administrator);
        bool writable = args.Contains("--control") && admin;
        var computer = new Computer { IsCpuEnabled = true, IsGpuEnabled = true,
            IsMotherboardEnabled = admin, IsStorageEnabled = true, IsControllerEnabled = admin };
        try
        {
            computer.Open();
            using var watchdog = new Timer(_ => { lock (Gate) Leases.Expire(); }, null, 500, 500);
            while (await ReadLineBounded() is { } line)
            {
                long requestId = 0;
                try
                {
                    using var doc = JsonDocument.Parse(line);
                    var root = doc.RootElement;
                    requestId = root.GetProperty("request_id").GetInt64();
                    object? report = null;
                    lock (Gate)
                    {
                        switch (root.GetProperty("op").GetString())
                        {
                            case "sample": report = Sample(computer, writable, admin); break;
                            case "restore_all":
                                if (!Leases.RestoreAll()) throw new InvalidOperationException("Some fan defaults could not be restored");
                                break;
                            case "set":
                                string id = root.GetProperty("channel").GetString() ?? "";
                                if (!writable || !Controls.TryGetValue(id, out var control))
                                    throw new InvalidOperationException("Channel is not available for software control");
                                var percent = root.GetProperty("percent");
                                if (percent.ValueKind == JsonValueKind.Null)
                                {
                                    control.SetDefault(); Leases.Forget(id);
                                }
                                else
                                {
                                    if (!Allowed.Contains(id)) throw new InvalidOperationException("Cooling channel is no longer available");
                                    float value = percent.GetSingle();
                                    ValidateDuty(value, Math.Max(30, control.MinSoftwareValue), Math.Min(100, control.MaxSoftwareValue));
                                    // Register rollback before entering the device API, including
                                    // the case where it partially writes and then throws.
                                    Leases.Applied(id, control.SetDefault);
                                    control.SetSoftware(value);
                                }
                                break;
                            default: throw new InvalidOperationException("Unknown hardware request");
                        }
                    }
                    Console.WriteLine(JsonSerializer.Serialize(new { protocol = 2, request_id = requestId, report }, Json));
                }
                catch (Exception e) { Console.WriteLine(JsonSerializer.Serialize(new { protocol = 2, request_id = requestId, error = e.Message }, Json)); }
            }
            return 0;
        }
        catch (Exception e) { Console.WriteLine(JsonSerializer.Serialize(new { protocol = 2, request_id = 0, error = e.Message }, Json)); return 1; }
        finally
        {
            lock (Gate) { Leases.RestoreAll(); computer.Close(); }
        }
    }

    internal static void ValidateDuty(float value, float min, float max)
    {
        if (!float.IsFinite(value) || !float.IsFinite(min) || !float.IsFinite(max) || min < 30 || max > 100 || min > max || value < min || value > max)
            throw new ArgumentOutOfRangeException(nameof(value), "Duty is outside the channel's allowed range");
    }

    internal static float? ReadingValue(HardwareType device, SensorType kind, float? value, bool admin)
    {
        // CPU register-backed readings can be exposed as zero by the library
        // without privileged access. Do not present those as measured values.
        bool requiresAccess = device == HardwareType.Cpu && kind is SensorType.Temperature or SensorType.Power or SensorType.Voltage;
        return (!requiresAccess || admin) && value is { } v && float.IsFinite(v) ? v : null;
    }

    private static async Task<string?> ReadLineBounded()
    {
        // ReadLineAsync would allocate an unbounded request before checking it.
        var text = new System.Text.StringBuilder();
        char[] one = new char[1];
        while (await Console.In.ReadAsync(one.AsMemory()) != 0)
        {
            if (one[0] == '\n') return text.ToString();
            if (text.Length == MaxRequest) throw new InvalidOperationException("Hardware request too large");
            text.Append(one[0]);
        }
        return null;
    }

    private static Report Sample(Computer computer, bool writable, bool admin)
    {
        var readings = new List<Reading>(); var channels = new List<Channel>();
        var errors = new List<string>();
        Allowed.Clear();
        void Visit(IHardware hardware)
        {
            try { hardware.Update(); }
            catch (Exception e) { errors.Add($"{hardware.Name}: {e.Message}"); return; }
            foreach (var sensor in hardware.Sensors)
            {
                string? kind = sensor.SensorType switch {
                    SensorType.Temperature => "temperature", SensorType.Fan => "fan", SensorType.Control => "control",
                    SensorType.Power => "power", SensorType.Voltage => "voltage", SensorType.Clock => "clock", SensorType.Load => "load", _ => null };
                string id = sensor.Identifier.ToString();
                if (kind is not null)
                    readings.Add(new(id, hardware.Name, sensor.Name, kind, ReadingValue(hardware.HardwareType, sensor.SensorType, sensor.Value, admin)));
                if (sensor.Control is { } control)
                {
                    Controls[id] = control;
                    bool pump = sensor.Name.Contains("pump", StringComparison.OrdinalIgnoreCase);
                    float min = Math.Max(30, control.MinSoftwareValue), max = Math.Min(100, control.MaxSoftwareValue);
                    bool valid = float.IsFinite(min) && min <= max && max == 100;
                    bool canWrite = writable && !pump && valid;
                    if (canWrite) Allowed.Add(id);
                    channels.Add(new(id, hardware.Name, sensor.Name, min, max, canWrite,
                        pump ? "Pump software control is disabled" : !valid ? "Invalid provider bounds" : !admin ? "Administrator rights required" : !writable ? "Read-only provider" : ""));
                }
            }
            foreach (var child in hardware.SubHardware) Visit(child);
        }
        foreach (var hardware in computer.Hardware) Visit(hardware);
        string notice = admin ? "Some motherboard/CPU sensors require a supported hardware access driver." : "Run the RustWTM service for administrator-only CPU/motherboard sensors and fan controls.";
        if (errors.Count != 0) notice += " " + string.Join("; ", errors.Take(5));
        return new("LibreHardwareMonitor 0.9.6", DateTimeOffset.UtcNow.ToUnixTimeMilliseconds(), readings, channels, notice);
    }
}
