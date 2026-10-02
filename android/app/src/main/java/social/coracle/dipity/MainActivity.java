package social.coracle.dipity;

import android.os.Bundle;
import com.getcapacitor.BridgeActivity;

public class MainActivity extends BridgeActivity {
    @Override
    public void onCreate(Bundle savedInstanceState) {
        // A plugin in the app module is not discovered; it is named here.
        registerPlugin(DipPlugin.class);

        super.onCreate(savedInstanceState);
    }
}
