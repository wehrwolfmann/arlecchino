// run() implements java.lang.Runnable and must stay locked.
public class j implements Runnable {
    int a;

    public void run() {
        a++;
    }

    void b() {
        run();
    }
}
