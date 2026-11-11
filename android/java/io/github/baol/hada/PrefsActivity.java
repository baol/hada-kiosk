// Preferences of the hada Android app: Home Assistant address, sign-in, rotation
// and start at boot. A plain Android screen, so the system keyboard just works.
// The dashboard (native code) opens it with open() and reads what was saved with
// takeResult() when it comes back to the front. The password is only passed on in
// memory, never stored.
package io.github.baol.hada;

import android.app.Activity;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.graphics.Color;
import android.os.Bundle;
import android.text.InputType;
import android.util.TypedValue;
import android.view.Gravity;
import android.view.View;
import android.view.WindowManager;
import android.widget.Button;
import android.widget.CheckBox;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.RadioButton;
import android.widget.RadioGroup;
import android.widget.ScrollView;
import android.widget.TextView;

public class PrefsActivity extends Activity {
    static final String PREFS = "hada";
    static final String START_AT_BOOT = "start_at_boot";
    private static final char SEP = '\u001f';
    private static String result; // taken by the dashboard

    /** Called by the dashboard (any thread). rotation: 0 auto, 1 portrait, 2 landscape, 3/4 upside down. */
    public static void open(Activity from, String url, String user, String token,
                            int rotation, String error, int signedIn) {
        Intent i = new Intent(from, PrefsActivity.class);
        i.putExtra("url", url);
        i.putExtra("user", user);
        i.putExtra("token", token);
        i.putExtra("rotation", rotation);
        i.putExtra("error", error);
        i.putExtra("signedIn", signedIn != 0);
        from.startActivity(i);
    }

    /** "saved␟url␟user␟password␟token␟rotation", "signout" or "cancel"; null if nothing new. */
    public static synchronized String takeResult() {
        String r = result;
        result = null;
        return r;
    }

    private static synchronized void setResult(String r) {
        result = r;
    }

    private EditText url, user, password, token;
    private RadioGroup orientation;
    private CheckBox flipped, startAtBoot;
    private boolean answered;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        getWindow().setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE);
        Intent in = getIntent();
        LinearLayout col = new LinearLayout(this);
        col.setOrientation(LinearLayout.VERTICAL);
        int pad = dp(24);
        col.setPadding(pad, pad, pad, pad);

        TextView title = text("hada", 30);
        title.setTypeface(null, android.graphics.Typeface.BOLD);
        col.addView(title);
        col.addView(note("Connect to Home Assistant"));

        String error = in.getStringExtra("error");
        if (error != null && !error.isEmpty()) {
            TextView e = text(error, 15);
            e.setTextColor(Color.rgb(0xff, 0x8a, 0x80));
            e.setPadding(0, dp(12), 0, 0);
            col.addView(e);
        }

        url = field(col, "Home Assistant address", in.getStringExtra("url"), "http://192.168.1.10:8123",
                InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_URI);
        col.addView(note("A dashboard address such as …:8123/lovelace/kiosk also opens that view. "
                + "If homeassistant.local does not work on this device, use the IP address."));
        user = field(col, "Username", in.getStringExtra("user"), "kiosk", InputType.TYPE_CLASS_TEXT);
        password = field(col, "Password", "", "optional",
                InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_PASSWORD);
        col.addView(note("Leave empty to sign in through trusted networks. "
                + "The password is used once and never stored."));
        token = field(col, "Long-lived access token", in.getStringExtra("token"),
                "optional, instead of username and password", InputType.TYPE_CLASS_TEXT);

        col.addView(label("Rotation"));
        orientation = new RadioGroup(this);
        orientation.setOrientation(RadioGroup.HORIZONTAL);
        String[] names = {"Auto", "Portrait", "Landscape"};
        for (int k = 0; k < names.length; k++) {
            RadioButton b = new RadioButton(this);
            b.setText(names[k]);
            b.setId(View.generateViewId());
            b.setTag(k);
            orientation.addView(b);
        }
        int rot = in.getIntExtra("rotation", 0);
        int base = rot == 3 ? 1 : rot == 4 ? 2 : rot;
        ((RadioButton) orientation.getChildAt(Math.max(0, Math.min(2, base)))).setChecked(true);
        col.addView(orientation);
        flipped = new CheckBox(this);
        flipped.setText("Upside down");
        flipped.setChecked(rot == 3 || rot == 4);
        col.addView(flipped);

        startAtBoot = new CheckBox(this);
        startAtBoot.setText("Start when the device starts");
        startAtBoot.setChecked(getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean(START_AT_BOOT, true));
        col.addView(startAtBoot);

        Button save = new Button(this);
        save.setText("Save & connect");
        save.setOnClickListener(v -> save());
        LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT);
        lp.topMargin = dp(16);
        col.addView(save, lp);
        if (in.getBooleanExtra("signedIn", false)) {
            Button out = new Button(this);
            out.setText("Sign out");
            out.setOnClickListener(v -> answer("signout"));
            col.addView(out, new LinearLayout.LayoutParams(
                    LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT));
        }
        col.addView(note("hada " + versionName()));

        ScrollView scroll = new ScrollView(this);
        scroll.setFillViewport(true);
        scroll.addView(col);
        setContentView(scroll);
    }

    private void save() {
        int checked = orientation.findViewById(orientation.getCheckedRadioButtonId()) != null
                ? (Integer) orientation.findViewById(orientation.getCheckedRadioButtonId()).getTag() : 0;
        int rot = checked == 0 ? 0 : (flipped.isChecked() ? checked + 2 : checked);
        getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
                .putBoolean(START_AT_BOOT, startAtBoot.isChecked()).apply();
        answer("saved" + SEP + url.getText() + SEP + user.getText() + SEP + password.getText()
                + SEP + token.getText() + SEP + rot);
    }

    private void answer(String r) {
        answered = true;
        setResult(r);
        password.setText("");
        finish();
    }

    @Override
    public void onBackPressed() {
        if (!answered) {
            answered = true;
            setResult("cancel");
        }
        super.onBackPressed();
    }

    private EditText field(LinearLayout col, String label, String value, String hint, int type) {
        col.addView(label(label));
        EditText e = new EditText(this);
        e.setSingleLine(true);
        e.setInputType(type);
        e.setHint(hint);
        e.setText(value == null ? "" : value);
        col.addView(e);
        return e;
    }

    private TextView label(String s) {
        TextView t = text(s, 14);
        t.setPadding(0, dp(16), 0, 0);
        t.setTypeface(null, android.graphics.Typeface.BOLD);
        return t;
    }

    private TextView note(String s) {
        TextView t = text(s, 12);
        t.setAlpha(0.7f);
        return t;
    }

    private TextView text(String s, int sp) {
        TextView t = new TextView(this);
        t.setText(s);
        t.setTextSize(TypedValue.COMPLEX_UNIT_SP, sp);
        t.setGravity(Gravity.START);
        return t;
    }

    private String versionName() {
        try {
            return getPackageManager().getPackageInfo(getPackageName(), 0).versionName;
        } catch (Exception e) {
            return "";
        }
    }

    private int dp(int v) {
        return (int) (v * getResources().getDisplayMetrics().density + 0.5f);
    }
}
