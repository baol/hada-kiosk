// Starts the dashboard when the device has booted, if "Start when the device
// starts" is on. A kiosk path for devices that do not allow choosing hada as
// the Home app (such as Fire tablets). Android 10 and newer block this.
package io.github.baol.hada;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;

public class BootReceiver extends BroadcastReceiver {
    @Override
    public void onReceive(Context context, Intent intent) {
        if (!Intent.ACTION_BOOT_COMPLETED.equals(intent.getAction())) {
            return;
        }
        boolean on = context.getSharedPreferences(PrefsActivity.PREFS, Context.MODE_PRIVATE)
                .getBoolean(PrefsActivity.START_AT_BOOT, true);
        if (on) {
            Intent start = new Intent(context, android.app.NativeActivity.class);
            start.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
            context.startActivity(start);
        }
    }
}
