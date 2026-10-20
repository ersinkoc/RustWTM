namespace Wtm.Hardware;

// The provider owns the final control lease. A service crash, closed input or
// missed renewals causes an attempt to return each channel to hardware default.
internal sealed class Leases(Func<long> clock)
{
    private sealed record Entry(Action Restore, long Expires);
    private readonly Dictionary<string, Entry> entries = [];
    public void Applied(string id, Action restore) => entries[id] = new(restore, clock() + 8000);
    public void Forget(string id) => entries.Remove(id);
    public void Expire()
    {
        foreach (var (id, entry) in entries.ToArray())
            if (clock() >= entry.Expires) Restore(id, entry);
    }
    private void Restore(string id, Entry entry)
    {
        // Keep a failed rollback so a later watchdog pass can retry it.
        try { entry.Restore(); entries.Remove(id); }
        catch (Exception e) { Console.Error.WriteLine($"Restore {id}: {e.Message}"); }
    }
    public bool RestoreAll()
    {
        foreach (var (id, entry) in entries.ToArray()) Restore(id, entry);
        return entries.Count == 0;
    }
    public int Count => entries.Count;

    public static void Test()
    {
        static void Check(bool condition) { if (!condition) throw new Exception("Lease assertion failed"); }
        long time = 0; int restored = 0;
        var lease = new Leases(() => time);
        lease.Applied("a", () => restored++);
        time = 7999; lease.Expire(); Check(restored == 0);
        lease.Applied("a", () => restored++); // A renewal extends only this channel.
        time = 8000; lease.Expire(); Check(restored == 0);
        time = 15999; lease.Expire(); Check(restored == 1 && lease.Count == 0);
        bool fail = true;
        lease.Applied("b", () => { if (fail) throw new Exception("injected restore failure"); restored++; });
        Check(!lease.RestoreAll() && lease.Count == 1);
        fail = false; Check(lease.RestoreAll() && restored == 2);
        Program.ValidateDuty(30,30,100); Program.ValidateDuty(100,30,100);
        foreach (float value in new[] {float.NaN,float.PositiveInfinity,29,101})
        {
            bool rejected=false;try {Program.ValidateDuty(value,30,100);}catch(ArgumentOutOfRangeException){rejected=true;}
            Check(rejected);
        }
        Check(Program.ReadingValue(LibreHardwareMonitor.Hardware.HardwareType.Cpu, LibreHardwareMonitor.Hardware.SensorType.Temperature, 0, false) is null);
        Check(Program.ReadingValue(LibreHardwareMonitor.Hardware.HardwareType.GpuNvidia, LibreHardwareMonitor.Hardware.SensorType.Fan, 0, false) == 0);
        Check(Program.ReadingValue(LibreHardwareMonitor.Hardware.HardwareType.GpuNvidia, LibreHardwareMonitor.Hardware.SensorType.Temperature, float.NaN, false) is null);
        Console.WriteLine("PASS: lease deadlines, renewal, expiry, rollback retry, shutdown, duty bounds and unavailable readings");
    }
}
