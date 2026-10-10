import importlib.util
from pathlib import Path
import unittest
spec = importlib.util.spec_from_file_location('handshake', Path(__file__).resolve().parents[1]/'experiments/sender_handshake.py')
m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)

class SenderHandshake(unittest.TestCase):
    def test_duplicate_before_ack_no_delivery_or_next_wait(self):
        s=m.Handshake()
        self.assertEqual(s.observe('538:9',False),'deliver')
        s.delivered(True)
        self.assertEqual(s.observe('538:9',False),'pending')
        self.assertEqual(s.handled('538:9',False),'pending')
        self.assertEqual(s.handled('538:9',True),'next-wait')
        self.assertEqual(s.observe('538:9',True),'superseded')
        self.assertEqual(s.observe('539:1',False),'deliver')
    def test_unknown_read_failed_send_or_wrong_receipt_stop(self):
        for failure in ('read','send','receipt','identity'):
            with self.subTest(failure=failure):
                s=m.Handshake()
                if failure=='read': self.assertEqual(s.observe('e',None),'stop')
                else:
                    s.observe('e',False)
                    if failure=='send': s.delivered(False)
                    elif failure=='receipt': s.handled('e',None)
                    else: s.handled('foreign',True)
                self.assertEqual(s.observe('next',False),'stop')
    def test_literal_tick_requires_completed_pass_not_send(self):
        s=m.Handshake(); self.assertEqual(s.observe('tick-pass-1',False),'deliver')
        s.delivered(True)
        self.assertEqual(s.handled('tick-pass-1',False),'pending')
        self.assertEqual(s.handled('tick-pass-1',True),'next-wait')
